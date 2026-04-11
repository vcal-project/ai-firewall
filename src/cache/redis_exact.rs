use crate::cache::exact::ExactCache;
use anyhow::Context;
use async_trait::async_trait;
use redis::{aio::ConnectionManager, AsyncCommands};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct RedisExactCache {
    conn: Arc<Mutex<ConnectionManager>>,
    ttl_seconds: usize,
}

impl RedisExactCache {
    pub fn new(conn: ConnectionManager, ttl_seconds: usize) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            ttl_seconds,
        }
    }
}

#[async_trait]
impl ExactCache for RedisExactCache {
    async fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        let start = Instant::now();

        let mut conn = self.conn.lock().await;

        let result: Option<String> = conn
            .get(key)
            .await
            .with_context(|| format!("redis GET failed for key '{}'", key))?;

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

        let mut conn = self.conn.lock().await;

        let _: () = conn
            .set_ex(key, value, self.ttl_seconds as u64)
            .await
            .with_context(|| format!("redis SETEX failed for key '{}'", key))?;

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
