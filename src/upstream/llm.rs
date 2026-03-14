use async_trait::async_trait;

use crate::types::openai::{ChatCompletionRequest, ChatCompletionResponse};

#[async_trait]
pub trait LlmUpstream: Send + Sync {
    async fn chat_completion(
        &self,
        req: &ChatCompletionRequest,
    ) -> anyhow::Result<ChatCompletionResponse>;
}
