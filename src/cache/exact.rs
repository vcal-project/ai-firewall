use async_trait::async_trait;

#[async_trait]
pub trait ExactCache: Send + Sync {
    async fn get(&self, key: &str) -> anyhow::Result<Option<String>>;
    async fn set(&self, key: &str, value: String) -> anyhow::Result<()>;
}
