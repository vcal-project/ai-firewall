use async_trait::async_trait;

use crate::{
    semantic::semantic_cache::{SemanticCache, SemanticLookupHit},
    types::openai::ChatCompletionResponse,
};

pub struct NoopSemanticCache;

#[async_trait]
impl SemanticCache for NoopSemanticCache {
    async fn lookup(
        &self,
        _model: &str,
        _normalized_prompt: &str,
        _compatibility_scope: &str,
        _privacy_cache_scope: Option<&str>,
        _privacy_placeholder_signature: Option<&str>,
    ) -> anyhow::Result<Option<SemanticLookupHit>> {
        Ok(None)
    }

    async fn store(
        &self,
        _model: &str,
        _normalized_prompt: &str,
        _response: &ChatCompletionResponse,
        _compatibility_scope: &str,
        _privacy_cache_scope: Option<&str>,
        _privacy_placeholder_signature: Option<&str>,
    ) -> anyhow::Result<Option<crate::embeddings::provider::EmbeddingUsage>> {
        Ok(None)
    }
}
