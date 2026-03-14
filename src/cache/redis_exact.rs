use crate::cache::exact::ExactCache;
use async_trait::async_trait;
use redis::{aio::ConnectionManager, AsyncCommands};
use std::sync::Arc;
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
        let mut conn = self.conn.lock().await;
        let raw: Option<String> = conn.get(key).await?;
        Ok(raw)
    }

    async fn set(&self, key: &str, value: String) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().await;
        let _: () = conn.set_ex(key, value, self.ttl_seconds as u64).await?;
        Ok(())
    }
}
