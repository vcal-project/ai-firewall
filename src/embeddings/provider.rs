use async_trait::async_trait;

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed_text(&self, input: &str) -> anyhow::Result<Vec<f32>>;
}
