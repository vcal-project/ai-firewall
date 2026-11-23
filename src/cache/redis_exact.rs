use crate::{cache::exact::ExactCache, metrics};
use anyhow::Context;
use async_trait::async_trait;
use redis::{aio::ConnectionManager, AsyncCommands};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::{
    sync::Mutex,
    time::{interval, timeout, MissedTickBehavior},
};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
#[error("redis {operation} timed out after {timeout_seconds}s")]
pub struct RedisOperationTimeout {
    operation: &'static str,
    timeout_seconds: u64,
}

fn redis_server_code_class(code: Option<&str>) -> Option<&'static str> {
    match code {
        Some("MISCONF") => Some("persistence_rejected"),
        Some("READONLY") => Some("write_rejected"),
        _ => None,
    }
}

fn redis_error_class(error: &redis::RedisError) -> &'static str {
    if let Some(class) = redis_server_code_class(error.code()) {
        return class;
    }
    if error.is_io_error() {
        "connection_error"
    } else {
        "redis_error"
    }
}

fn redis_health_error_class(error: &anyhow::Error) -> &'static str {
    if error.downcast_ref::<RedisOperationTimeout>().is_some() {
        return "timeout";
    }

    for source in error.chain() {
        if let Some(redis_error) = source.downcast_ref::<redis::RedisError>() {
            return redis_error_class(redis_error);
        }
    }

    "client_unavailable"
}

#[derive(Clone)]
pub struct RedisExactCache {
    client: redis::Client,
    conn: Arc<Mutex<Option<ConnectionManager>>>,
    reconnect_inflight: Arc<AtomicBool>,
    ttl_seconds: usize,
    operation_timeout: Duration,
    health_probe_key: String,
}

impl RedisExactCache {
    pub fn new(
        client: redis::Client,
        conn: ConnectionManager,
        ttl_seconds: usize,
        operation_timeout: Duration,
    ) -> Self {
        Self::with_connection(client, Some(conn), ttl_seconds, operation_timeout)
    }

    pub fn new_disconnected(
        client: redis::Client,
        ttl_seconds: usize,
        operation_timeout: Duration,
    ) -> Self {
        Self::with_connection(client, None, ttl_seconds, operation_timeout)
    }

    fn with_connection(
        client: redis::Client,
        conn: Option<ConnectionManager>,
        ttl_seconds: usize,
        operation_timeout: Duration,
    ) -> Self {
        Self {
            client,
            conn: Arc::new(Mutex::new(conn)),
            reconnect_inflight: Arc::new(AtomicBool::new(false)),
            ttl_seconds,
            operation_timeout,
            health_probe_key: format!("aif:redis-health:{}", Uuid::new_v4()),
        }
    }

    async fn current_connection(&self) -> anyhow::Result<ConnectionManager> {
        self.conn
            .lock()
            .await
            .clone()
            .context("redis connection manager not yet initialized")
    }

    pub fn start_health_monitor(self: &Arc<Self>, available: Arc<AtomicBool>) {
        metrics::REDIS_WRITE_HEALTHY.set(-1);
        let cache = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut last_probe_healthy = available.load(Ordering::Acquire);
            let mut ticks = interval(Duration::from_secs(5));
            ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
            ticks.tick().await;

            loop {
                ticks.tick().await;
                let Some(cache) = cache.upgrade() else {
                    break;
                };

                match cache.probe_write().await {
                    Ok(()) => {
                        metrics::REDIS_WRITE_PROBES_TOTAL
                            .with_label_values(&["success"])
                            .inc();
                        metrics::REDIS_WRITE_HEALTHY.set(1);
                        available.store(true, Ordering::Release);
                        if !last_probe_healthy {
                            last_probe_healthy = true;
                            metrics::REDIS_WRITE_TRANSITIONS_TOTAL
                                .with_label_values(&["available"])
                                .inc();
                            tracing::info!(
                                redis_health = "available",
                                probe = "SETEX",
                                "Redis write health restored; dependency marked available"
                            );
                        }
                    }
                    Err(error) => {
                        let error_class = redis_health_error_class(&error);
                        metrics::REDIS_WRITE_PROBES_TOTAL
                            .with_label_values(&[error_class])
                            .inc();
                        metrics::REDIS_WRITE_HEALTHY.set(0);
                        available.store(false, Ordering::Release);
                        if last_probe_healthy {
                            last_probe_healthy = false;
                            metrics::REDIS_WRITE_TRANSITIONS_TOTAL
                                .with_label_values(&["unavailable"])
                                .inc();
                            tracing::warn!(
                                redis_health = "unavailable",
                                probe = "SETEX",
                                error_class,
                                error = %error,
                                "Redis write health failed; dependency marked unavailable"
                            );
                        }
                    }
                }
            }
        });
    }

    async fn probe_write(&self) -> anyhow::Result<()> {
        let result = timeout(self.operation_timeout, async {
            let mut conn = self.current_connection().await?;
            let _: () = conn
                .set_ex(self.health_probe_key.as_str(), "ok", 15)
                .await
                .context("redis health SETEX failed")?;
            Ok::<(), anyhow::Error>(())
        })
        .await;

        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.trigger_reconnect("HEALTH_SETEX");
                Err(error)
            }
            Err(_) => {
                self.trigger_reconnect("HEALTH_SETEX");
                Err(anyhow::Error::new(RedisOperationTimeout {
                    operation: "HEALTH_SETEX",
                    timeout_seconds: self.operation_timeout.as_secs(),
                }))
            }
        }
    }

    fn trigger_reconnect(&self, operation: &'static str) {
        if self.reconnect_inflight.swap(true, Ordering::AcqRel) {
            return;
        }

        let client = self.client.clone();
        let conn = Arc::clone(&self.conn);
        let reconnect_inflight = Arc::clone(&self.reconnect_inflight);
        let reconnect_timeout = self.operation_timeout;

        tokio::spawn(async move {
            let reconnect_result = timeout(reconnect_timeout, ConnectionManager::new(client)).await;

            match reconnect_result {
                Ok(Ok(new_conn)) => {
                    let mut current = conn.lock().await;
                    *current = Some(new_conn);
                    metrics::REDIS_RECONNECT_ATTEMPTS_TOTAL
                        .with_label_values(&["success"])
                        .inc();
                    tracing::debug!(
                        failed_operation = operation,
                        "Redis exact-cache connection manager refreshed; write health unverified"
                    );
                }
                Ok(Err(error)) => {
                    let error_class = redis_error_class(&error);
                    metrics::REDIS_RECONNECT_ATTEMPTS_TOTAL
                        .with_label_values(&["error"])
                        .inc();
                    tracing::warn!(
                        failed_operation = operation,
                        error_class,
                        error = %error,
                        "Redis exact-cache background reconnect failed"
                    );
                }
                Err(_) => {
                    metrics::REDIS_RECONNECT_ATTEMPTS_TOTAL
                        .with_label_values(&["timeout"])
                        .inc();
                    tracing::warn!(
                        failed_operation = operation,
                        error_class = "timeout",
                        timeout_seconds = reconnect_timeout.as_secs(),
                        "Redis exact-cache background reconnect timed out"
                    );
                }
            }

            reconnect_inflight.store(false, Ordering::Release);
        });
    }
}

#[async_trait]
impl ExactCache for RedisExactCache {
    async fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        let start = Instant::now();

        let operation_result = timeout(self.operation_timeout, async {
            let mut conn = self.current_connection().await?;
            conn.get(key)
                .await
                .with_context(|| format!("redis GET failed for key '{}'", key))
        })
        .await;

        let result = match operation_result {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                self.trigger_reconnect("GET");
                return Err(error);
            }
            Err(_) => {
                self.trigger_reconnect("GET");
                return Err(anyhow::Error::new(RedisOperationTimeout {
                    operation: "GET",
                    timeout_seconds: self.operation_timeout.as_secs(),
                }));
            }
        };

        let elapsed = start.elapsed().as_secs_f64();

        match &result {
            Some(_) => tracing::debug!(
                key = %key,
                latency_seconds = elapsed,
                "exact cache hit (redis)"
            ),
            None => tracing::debug!(
                key = %key,
                latency_seconds = elapsed,
                "exact cache miss (redis)"
            ),
        }

        Ok(result)
    }

    async fn set(&self, key: &str, value: String) -> anyhow::Result<()> {
        let start = Instant::now();

        let operation_result = timeout(self.operation_timeout, async {
            let mut conn = self.current_connection().await?;
            let _: () = conn
                .set_ex(key, value, self.ttl_seconds as u64)
                .await
                .with_context(|| format!("redis SETEX failed for key '{}'", key))?;
            Ok::<(), anyhow::Error>(())
        })
        .await;

        match operation_result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.trigger_reconnect("SETEX");
                return Err(error);
            }
            Err(_) => {
                self.trigger_reconnect("SETEX");
                return Err(anyhow::Error::new(RedisOperationTimeout {
                    operation: "SETEX",
                    timeout_seconds: self.operation_timeout.as_secs(),
                }));
            }
        }

        let elapsed = start.elapsed().as_secs_f64();

        tracing::debug!(
            key = %key,
            ttl_seconds = self.ttl_seconds,
            latency_seconds = elapsed,
            "exact cache set (redis)"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redis_probe_timeout_uses_stable_class() {
        let error = anyhow::Error::new(RedisOperationTimeout {
            operation: "HEALTH_SETEX",
            timeout_seconds: 2,
        });
        assert_eq!(redis_health_error_class(&error), "timeout");
    }

    #[test]
    fn known_redis_rejections_have_stable_classes() {
        assert_eq!(
            redis_server_code_class(Some("MISCONF")),
            Some("persistence_rejected")
        );
        assert_eq!(
            redis_server_code_class(Some("READONLY")),
            Some("write_rejected")
        );
        assert_eq!(redis_server_code_class(Some("OTHER")), None);
    }

    #[test]
    fn non_redis_probe_error_is_not_a_high_cardinality_label() {
        let error = anyhow::anyhow!("redis connection manager not yet initialized");
        assert_eq!(redis_health_error_class(&error), "client_unavailable");
    }
}
