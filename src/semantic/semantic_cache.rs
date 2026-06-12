use async_trait::async_trait;

use crate::{embeddings::provider::EmbeddingUsage, types::openai::ChatCompletionResponse};

#[derive(Debug, Clone)]
pub struct SemanticLookupHit {
    pub response: ChatCompletionResponse,
    pub embedding_usage: Option<EmbeddingUsage>,
}

#[async_trait]
pub trait SemanticCache: Send + Sync {
    async fn lookup(
        &self,
        model: &str,
        normalized_prompt: &str,
        privacy_placeholder_signature: Option<&str>,
    ) -> anyhow::Result<Option<SemanticLookupHit>>;

    async fn store(
        &self,
        model: &str,
        normalized_prompt: &str,
        response: &ChatCompletionResponse,
        privacy_placeholder_signature: Option<&str>,
    ) -> anyhow::Result<Option<EmbeddingUsage>>;
}
