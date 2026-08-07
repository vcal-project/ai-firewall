use crate::cache::exact::ExactCache;
use anyhow::Context;
use async_trait::async_trait;
use redis::{aio::ConnectionManager, AsyncCommands};
use std::sync::Arc;
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
    conn: Arc<Mutex<ConnectionManager>>,
    ttl_seconds: usize,
    operation_timeout: Duration,
}

impl RedisExactCache {
    pub fn new(conn: ConnectionManager, ttl_seconds: usize, operation_timeout: Duration) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            ttl_seconds,
            operation_timeout,
        }
    }
}

#[async_trait]
impl ExactCache for RedisExactCache {
    async fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        let start = Instant::now();

        let result: Option<String> = timeout(self.operation_timeout, async {
            let mut conn = self.conn.lock().await;
            conn.get(key)
                .await
                .with_context(|| format!("redis GET failed for key '{}'", key))
        })
        .await
        .map_err(|_| {
            anyhow::Error::new(RedisOperationTimeout {
                operation: "GET",
                timeout_seconds: self.operation_timeout.as_secs(),
            })
        })??;

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

        timeout(self.operation_timeout, async {
            let mut conn = self.conn.lock().await;
            let _: () = conn
                .set_ex(key, value, self.ttl_seconds as u64)
                .await
                .with_context(|| format!("redis SETEX failed for key '{}'", key))?;
            Ok::<(), anyhow::Error>(())
        })
        .await
        .map_err(|_| {
            anyhow::Error::new(RedisOperationTimeout {
                operation: "SETEX",
                timeout_seconds: self.operation_timeout.as_secs(),
            })
        })??;

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
