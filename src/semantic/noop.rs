use async_trait::async_trait;

use crate::{semantic::semantic_cache::SemanticCache, types::openai::ChatCompletionResponse};

pub struct NoopSemanticCache;

#[async_trait]
impl SemanticCache for NoopSemanticCache {
    async fn lookup(
        &self,
        _model: &str,
        _normalized_prompt: &str,
    ) -> anyhow::Result<Option<ChatCompletionResponse>> {
        Ok(None)
    }

    async fn store(
        &self,
        _model: &str,
        _normalized_prompt: &str,
        _response: &ChatCompletionResponse,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}
