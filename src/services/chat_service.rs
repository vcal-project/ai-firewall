use std::{collections::HashMap, sync::Arc};

use crate::{
    cache::exact::ExactCache,
    config::ModelPrice,
    core::{
        hashing::sha256_hex,
        normalize::{normalize_chat_request, semantic_text_from_request},
        pricing::estimate_micro_usd_saved,
    },
    error::AppError,
    metrics,
    semantic::semantic_cache::SemanticCache,
    types::openai::{ChatCompletionRequest, ChatCompletionResponse},
    upstream::llm::LlmUpstream,
};

pub struct ChatService {
    exact_cache: Arc<dyn ExactCache>,
    semantic_cache: Arc<dyn SemanticCache>,
    upstream: Arc<dyn LlmUpstream>,
    semantic_cache_enabled: bool,
    model_prices: HashMap<String, ModelPrice>,
}

impl ChatService {
    pub fn new(
        exact_cache: Arc<dyn ExactCache>,
        semantic_cache: Arc<dyn SemanticCache>,
        upstream: Arc<dyn LlmUpstream>,
        semantic_cache_enabled: bool,
        model_prices: HashMap<String, ModelPrice>,
    ) -> Self {
        Self {
            exact_cache,
            semantic_cache,
            upstream,
            semantic_cache_enabled,
            model_prices,
        }
    }

    pub async fn handle(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, AppError> {
        self.validate(&req)?;

        if req.stream.unwrap_or(false) {
            return self.forward_only(req).await;
        }

        let normalized = normalize_chat_request(&req)
            .map_err(|e| AppError::BadRequest(format!("normalize failed: {e}")))?;
        let semantic_text = semantic_text_from_request(&req);

        let exact_key = self.exact_cache_key(&normalized);

        if let Some(raw) = self
            .exact_cache
            .get(&exact_key)
            .await
            .map_err(|e| AppError::Internal(format!("exact cache get failed: {e}")))?
        {
            let hit: ChatCompletionResponse = serde_json::from_str(&raw)
                .map_err(|e| AppError::Internal(format!("cached response decode failed: {e}")))?;

            metrics::CACHE_EXACT_HITS.inc();
            self.record_savings(&hit);
            return Ok(hit);
        }

        if self.semantic_cache_enabled && self.semantic_eligible(&req) {
            if let Some(hit) = self
                .semantic_cache
                .lookup(&req.model, &semantic_text)
                .await
                .map_err(|e| AppError::Internal(format!("semantic lookup failed: {e}")))?
            {
                metrics::CACHE_SEMANTIC_HITS.inc();
                self.record_savings(&hit);

                if let Ok(raw) = serde_json::to_string(&hit) {
                    let _ = self.exact_cache.set(&exact_key, raw).await;
                }

                return Ok(hit);
            }
        }

        metrics::CACHE_MISSES.inc();
        metrics::UPSTREAM_CALLS.inc();

        let response = self
            .upstream
            .chat_completion(&req)
            .await
            .map_err(|e| AppError::Upstream(e.to_string()))?;

        let raw = serde_json::to_string(&response)
            .map_err(|e| AppError::Internal(format!("response encode failed: {e}")))?;

        self.exact_cache
            .set(&exact_key, raw)
            .await
            .map_err(|e| AppError::Internal(format!("exact cache set failed: {e}")))?;

        if self.semantic_cache_enabled && self.semantic_eligible(&req) {
            let _ = self
                .semantic_cache
                .store(&req.model, &semantic_text, &response)
                .await;
        }

        Ok(response)
    }

    fn validate(&self, req: &ChatCompletionRequest) -> Result<(), AppError> {
        if req.model.trim().is_empty() {
            return Err(AppError::BadRequest("model must not be empty".into()));
        }
        if req.messages.is_empty() {
            return Err(AppError::BadRequest("messages must not be empty".into()));
        }
        Ok(())
    }

    fn semantic_eligible(&self, req: &ChatCompletionRequest) -> bool {
        if req.stream.unwrap_or(false) {
            return false;
        }

        if req.extra.contains_key("tools") {
            return false;
        }

        if req.extra.contains_key("response_format") {
            return false;
        }

        true
    }

    async fn forward_only(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, AppError> {
        metrics::UPSTREAM_CALLS.inc();

        self.upstream
            .chat_completion(&req)
            .await
            .map_err(|e| AppError::Upstream(e.to_string()))
    }

    fn exact_cache_key(&self, normalized: &str) -> String {
        format!("chatcmpl:v1:{}", sha256_hex(normalized))
    }

    fn record_savings(&self, response: &ChatCompletionResponse) {
        if let Some(usage) = &response.usage {
            metrics::TOKENS_SAVED.inc_by(usage.total_tokens as u64);

            let saved = estimate_micro_usd_saved(&response.model, usage, &self.model_prices);
            metrics::COST_SAVED_MICRO_USD.inc_by(saved);

            if saved == 0 {
                tracing::debug!(
                    "no configured model_price for model '{}'; cost_saved not incremented",
                    response.model
                );
            }
        }
    }
}
