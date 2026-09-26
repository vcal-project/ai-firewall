use crate::cache::exact::ExactCache;
use anyhow::Context;
use async_trait::async_trait;
use redis::{aio::ConnectionManager, AsyncCommands};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::{sync::Mutex, time::timeout};

#[derive(Debug, thiserror::Error)]
#[error("redis {operation} timed out after {timeout_seconds}s")]
pub struct RedisOperationTimeout {
    operation: &'static str,
    timeout_seconds: u64,
}

#[derive(Clone)]
pub struct RedisExactCache {
    client: redis::Client,
    conn: Arc<Mutex<ConnectionManager>>,
    reconnect_inflight: Arc<AtomicBool>,
    ttl_seconds: usize,
    operation_timeout: Duration,
}

impl RedisExactCache {
    pub fn new(
        client: redis::Client,
        conn: ConnectionManager,
        ttl_seconds: usize,
        operation_timeout: Duration,
    ) -> Self {
        Self {
            client,
            conn: Arc::new(Mutex::new(conn)),
            reconnect_inflight: Arc::new(AtomicBool::new(false)),
            ttl_seconds,
            operation_timeout,
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
                    *current = new_conn;
                    tracing::info!(
                        failed_operation = operation,
                        "Redis exact-cache connection manager reconnected"
                    );
                }
                Ok(Err(error)) => {
                    tracing::warn!(
                        failed_operation = operation,
                        error = %error,
                        "Redis exact-cache background reconnect failed"
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        failed_operation = operation,
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
            let mut conn = self.conn.lock().await;
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
            let mut conn = self.conn.lock().await;
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
