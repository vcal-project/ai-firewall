use async_trait::async_trait;

use crate::types::openai::ChatCompletionResponse;

#[async_trait]
pub trait SemanticCache: Send + Sync {
    async fn lookup(
        &self,
        model: &str,
        normalized_prompt: &str,
    ) -> anyhow::Result<Option<ChatCompletionResponse>>;

    async fn store(
        &self,
        model: &str,
        normalized_prompt: &str,
        response: &ChatCompletionResponse,
    ) -> anyhow::Result<()>;
}
