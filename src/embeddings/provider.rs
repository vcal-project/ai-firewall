use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct EmbeddingResult {
    pub embedding: Vec<f32>,
    pub usage: Option<EmbeddingUsage>,
    #[allow(dead_code)]
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EmbeddingUsage {
    pub prompt_tokens: u32,
    #[allow(dead_code)]
    pub total_tokens: u32,
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed_text(&self, input: &str) -> anyhow::Result<EmbeddingResult>;
}
