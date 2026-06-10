use std::{collections::HashMap, sync::Arc};

use crate::{
    cache::exact::ExactCache,
    config::{EmbeddingPrice, ModelPrice},
    core::{
        hashing::sha256_hex,
        normalize::{normalize_chat_request, semantic_text_from_request},
        pricing::{estimate_embedding_micro_usd, estimate_micro_usd_saved},
    },
    error::AppError,
    metrics::{
        self, CACHE_TYPE_EXACT, CACHE_TYPE_SEMANTIC, COST_TYPE_CHAT, COST_TYPE_EMBEDDING,
        EMBEDDING_OPERATION_LOOKUP, EMBEDDING_OPERATION_STORE,
    },
    semantic::semantic_cache::SemanticCache,
    types::openai::{ChatCompletionRequest, ChatCompletionResponse},
    upstream::llm::LlmUpstream,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct CacheControl {
    pub bypass_lookup: bool,
    pub bypass_store: bool,
}

pub struct ChatService {
    exact_cache: Arc<dyn ExactCache>,
    semantic_cache: Arc<dyn SemanticCache>,
    upstream: Arc<dyn LlmUpstream>,
    exact_cache_enabled: bool,
    exact_cache_fail_open: bool,
    exact_cache_store_enabled: bool,
    semantic_cache_enabled: bool,
    semantic_cache_fail_open: bool,
    semantic_cache_store_enabled: bool,
    max_prompt_chars: Option<usize>,
    model_prices: HashMap<String, ModelPrice>,
    embedding_price: Option<EmbeddingPrice>,
}

#[derive(Clone, Debug)]
pub struct ChatServiceSettings {
    pub semantic_cache_enabled: bool,
    pub exact_cache_enabled: bool,
    pub exact_cache_fail_open: bool,
    pub exact_cache_store_enabled: bool,
    pub semantic_cache_store_enabled: bool,
    pub semantic_cache_fail_open: bool,
    pub max_prompt_chars: Option<usize>,
}

fn estimate_prompt_chars(req: &ChatCompletionRequest) -> usize {
    req.messages
        .iter()
        .map(|message| message.content.to_string().chars().count())
        .sum()
}

impl ChatService {
    pub fn new(
        exact_cache: Arc<dyn ExactCache>,
        semantic_cache: Arc<dyn SemanticCache>,
        upstream: Arc<dyn LlmUpstream>,
        settings: ChatServiceSettings,
        model_prices: HashMap<String, ModelPrice>,
        embedding_price: Option<EmbeddingPrice>,
    ) -> Self {
        Self {
            exact_cache,
            semantic_cache,
            upstream,
            exact_cache_enabled: settings.exact_cache_enabled,
            exact_cache_fail_open: settings.exact_cache_fail_open,
            exact_cache_store_enabled: settings.exact_cache_store_enabled,
            semantic_cache_enabled: settings.semantic_cache_enabled,
            semantic_cache_fail_open: settings.semantic_cache_fail_open,
            semantic_cache_store_enabled: settings.semantic_cache_store_enabled,
            max_prompt_chars: settings.max_prompt_chars,
            model_prices,
            embedding_price,
        }
    }

    #[cfg(test)]
    pub async fn handle(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, AppError> {
        self.handle_with_cache_control(req, CacheControl::default())
            .await
    }

    pub async fn handle_with_cache_control(
        &self,
        req: ChatCompletionRequest,
        cache_control: CacheControl,
    ) -> Result<ChatCompletionResponse, AppError> {
        self.validate(&req)?;

        if req.stream.unwrap_or(false) {
            tracing::debug!(model = %req.normalized_model(), "stream request bypasses cache layers");
            return self.forward_only(req).await;
        }

        let normalized = normalize_chat_request(&req)
            .map_err(|e| AppError::bad_request(format!("normalize failed: {e}")))?;
        let semantic_text = semantic_text_from_request(&req);

        let exact_key = self.exact_cache_key(&normalized);

        if self.exact_cache_enabled && !cache_control.bypass_lookup {
            match self.exact_cache.get(&exact_key).await {
                Ok(Some(raw)) => {
                    let hit: ChatCompletionResponse = serde_json::from_str(&raw).map_err(|e| {
                        AppError::internal(format!("cached response decode failed: {e}"))
                    })?;

                    metrics::CACHE_EXACT_HITS.inc();
                    self.record_exact_hit_savings(&hit);

                    tracing::debug!(
                        model = %req.normalized_model(),
                        cache_key = %exact_key,
                        "exact cache hit"
                    );

                    return Ok(hit);
                }
                Ok(None) => {}
                Err(e) if self.exact_cache_fail_open => {
                    tracing::warn!(
                        model = %req.normalized_model(),
                        error = %e,
                        "exact cache lookup failed; exact_cache_fail_open=true so request continues"
                    );
                }
                Err(e) => {
                    return Err(AppError::internal(format!("exact cache get failed: {e}")));
                }
            }
        } else if cache_control.bypass_lookup {
            tracing::debug!(
                model = %req.normalized_model(),
                "cache lookup bypass requested"
            );
        }

        if self.semantic_cache_enabled
            && !cache_control.bypass_lookup
            && self.semantic_eligible(&req)
        {
            match self
                .semantic_cache
                .lookup(req.normalized_model(), &semantic_text)
                .await
            {
                Ok(Some(hit)) => {
                    metrics::CACHE_SEMANTIC_HITS.inc();

                    self.record_semantic_hit_savings(
                        &hit.response,
                        hit.embedding_usage
                            .as_ref()
                            .map(|u| u.prompt_tokens)
                            .unwrap_or(0),
                    );

                    tracing::debug!(
                        model = %req.normalized_model(),
                        "semantic cache hit"
                    );

                    if self.exact_cache_enabled
                        && self.exact_cache_store_enabled
                        && !cache_control.bypass_store
                    {
                        if let Ok(raw) = serde_json::to_string(&hit.response) {
                            match self.exact_cache.set(&exact_key, raw).await {
                                Ok(()) => {}
                                Err(e) if self.exact_cache_fail_open => {
                                    tracing::debug!(
                                        "failed to warm exact cache from semantic hit: {e}"
                                    );
                                }
                                Err(e) => {
                                    return Err(AppError::internal(format!(
                                        "exact cache set failed while warming semantic hit: {e}"
                                    )));
                                }
                            }
                        } else {
                            tracing::debug!(
                                "failed to serialize semantic-hit response for exact cache warming"
                            );
                        }
                    }
                    return Ok(hit.response);
                }

                Ok(None) => {}

                Err(e) if self.semantic_cache_fail_open => {
                    self.record_semantic_skip("lookup_error");

                    tracing::warn!(
                        model = %req.normalized_model(),
                        error = %e,
                        "semantic lookup failed; skipping semantic cache and continuing upstream"
                    );
                }

                Err(e) => {
                    return Err(AppError::semantic_provider(format!(
                        "semantic lookup failed and semantic_cache_fail_open=false: {e}"
                    )));
                }
            }
        } else if self.semantic_cache_enabled {
            self.record_semantic_skip("ineligible_request");

            tracing::debug!(
                model = %req.normalized_model(),
                "semantic cache skipped because request is ineligible"
            );
        }

        metrics::CACHE_MISSES.inc();

        tracing::debug!(
            model = %req.normalized_model(),
            "cache miss; forwarding request upstream"
        );

        let response = self.upstream.chat_completion(&req).await?;

        self.record_upstream_model_cost(&response);

        let raw = serde_json::to_string(&response)
            .map_err(|e| AppError::internal(format!("response encode failed: {e}")))?;

        if self.exact_cache_enabled && self.exact_cache_store_enabled && !cache_control.bypass_store
        {
            match self.exact_cache.set(&exact_key, raw).await {
                Ok(()) => {}
                Err(e) if self.exact_cache_fail_open => {
                    tracing::warn!(
                        model = %req.normalized_model(),
                        error = %e,
                        "exact cache store failed; exact_cache_fail_open=true so response is returned"
                    );
                }
                Err(e) => {
                    return Err(AppError::internal(format!("exact cache set failed: {e}")));
                }
            }
        }

        if self.semantic_cache_enabled
            && self.semantic_cache_store_enabled
            && !cache_control.bypass_store
            && self.semantic_eligible(&req)
        {
            match self
                .semantic_cache
                .store(req.normalized_model(), &semantic_text, &response)
                .await
            {
                Ok(embedding_usage) => {
                    if let Some(usage) = embedding_usage {
                        self.record_embedding_overhead(
                            req.normalized_model(),
                            EMBEDDING_OPERATION_STORE,
                            usage.prompt_tokens,
                        );
                    }
                }

                Err(e) if self.semantic_cache_fail_open => {
                    self.record_semantic_skip("store_error");

                    tracing::warn!(
                        model = %req.normalized_model(),
                        error = %e,
                        "semantic store failed; response returned without semantic cache write"
                    );
                }

                Err(e) => {
                    self.record_semantic_skip("store_error");

                    return Err(AppError::semantic_provider(format!(
                        "semantic store failed and semantic_cache_fail_open=false: {e}"
                    )));
                }
            }
        }

        Ok(response)
    }

    fn validate(&self, req: &ChatCompletionRequest) -> Result<(), AppError> {
        if req.normalized_model().is_empty() {
            return Err(AppError::bad_request("model must not be empty"));
        }

        if req.messages.is_empty() {
            return Err(AppError::bad_request("messages must not be empty"));
        }

        if let Some(max_prompt_chars) = self.max_prompt_chars {
            let prompt_chars = estimate_prompt_chars(req);
            if prompt_chars > max_prompt_chars {
                return Err(AppError::payload_too_large(format!(
                    "prompt size exceeds max_prompt_chars ({} > {})",
                    prompt_chars, max_prompt_chars
                )));
            }
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
        let response = self.upstream.chat_completion(&req).await?;
        self.record_upstream_model_cost(&response);
        Ok(response)
    }

    fn exact_cache_key(&self, normalized: &str) -> String {
        format!("chatcmpl:v1:{}", sha256_hex(normalized))
    }

    fn record_upstream_model_cost(&self, response: &ChatCompletionResponse) {
        let Some(usage) = &response.usage else {
            tracing::debug!(
                model = %response.model,
                "upstream response has no usage; per-model cost metrics not incremented"
            );
            return;
        };

        let model = response.model.as_str();

        metrics::MODEL_REQUESTS_TOTAL
            .with_label_values(&[model])
            .inc();

        metrics::MODEL_INPUT_TOKENS_TOTAL
            .with_label_values(&[model])
            .inc_by(usage.prompt_tokens as u64);

        metrics::MODEL_OUTPUT_TOKENS_TOTAL
            .with_label_values(&[model])
            .inc_by(usage.completion_tokens as u64);

        let cost = estimate_micro_usd_saved(&response.model, usage, &self.model_prices);

        metrics::MODEL_COST_MICRO_USD_TOTAL
            .with_label_values(&[model])
            .inc_by(cost);

        metrics::REQUEST_COST_MICRO_USD_TOTAL
            .with_label_values(&[model, COST_TYPE_CHAT])
            .inc_by(cost);

        if cost == 0 {
            tracing::debug!(
                "no configured model_price for model '{}'; per-model cost metrics recorded tokens only",
                response.model
            );
        }
    }

    fn record_semantic_skip(&self, reason: &'static str) {
        metrics::SEMANTIC_SKIPS_TOTAL
            .with_label_values(&[reason])
            .inc();
    }

    fn record_exact_hit_savings(&self, response: &ChatCompletionResponse) {
        metrics::CACHE_HITS_TOTAL
            .with_label_values(&[response.model.as_str(), CACHE_TYPE_EXACT])
            .inc();

        let Some(usage) = &response.usage else {
            return;
        };

        metrics::TOKENS_SAVED.inc_by(usage.total_tokens as u64);

        let saved = estimate_micro_usd_saved(&response.model, usage, &self.model_prices);

        metrics::CHAT_COST_SAVED_MICRO_USD.inc_by(saved);
        metrics::COST_SAVED_MICRO_USD.inc_by(saved);

        metrics::GROSS_SAVED_MICRO_USD_TOTAL
            .with_label_values(&[response.model.as_str(), CACHE_TYPE_EXACT])
            .inc_by(saved);

        metrics::NET_SAVED_MICRO_USD_TOTAL
            .with_label_values(&[response.model.as_str(), CACHE_TYPE_EXACT])
            .inc_by(saved);

        if saved == 0 {
            tracing::debug!(
                "no configured model_price for model '{}'; exact-hit cost_saved not incremented",
                response.model
            );
        }
    }

    fn record_semantic_hit_savings(
        &self,
        response: &ChatCompletionResponse,
        embedding_prompt_tokens: u32,
    ) {
        metrics::CACHE_HITS_TOTAL
            .with_label_values(&[response.model.as_str(), CACHE_TYPE_SEMANTIC])
            .inc();

        let Some(usage) = &response.usage else {
            return;
        };

        let gross_saved = estimate_micro_usd_saved(&response.model, usage, &self.model_prices);
        let embedding_cost =
            estimate_embedding_micro_usd(embedding_prompt_tokens, self.embedding_price.as_ref());
        let net_saved = gross_saved.saturating_sub(embedding_cost);

        metrics::TOKENS_SAVED.inc_by(usage.total_tokens as u64);

        metrics::CHAT_COST_SAVED_MICRO_USD.inc_by(gross_saved);
        metrics::COST_SAVED_MICRO_USD.inc_by(net_saved);

        metrics::GROSS_SAVED_MICRO_USD_TOTAL
            .with_label_values(&[response.model.as_str(), CACHE_TYPE_SEMANTIC])
            .inc_by(gross_saved);

        metrics::NET_SAVED_MICRO_USD_TOTAL
            .with_label_values(&[response.model.as_str(), CACHE_TYPE_SEMANTIC])
            .inc_by(net_saved);

        self.record_embedding_overhead(
            response.model.as_str(),
            EMBEDDING_OPERATION_LOOKUP,
            embedding_prompt_tokens,
        );

        if gross_saved == 0 {
            tracing::debug!(
                "no configured model_price for model '{}'; semantic-hit cost_saved not incremented",
                response.model
            );
        } else {
            tracing::debug!(
                model = %response.model,
                gross_saved_micro_usd = gross_saved,
                embedding_cost_micro_usd = embedding_cost,
                net_saved_micro_usd = net_saved,
                "recorded semantic-hit net savings"
            );
        }
    }

    fn record_embedding_overhead(
        &self,
        chat_model: &str,
        operation: &'static str,
        embedding_prompt_tokens: u32,
    ) {
        let embedding_cost =
            estimate_embedding_micro_usd(embedding_prompt_tokens, self.embedding_price.as_ref());

        metrics::EMBEDDING_COST_MICRO_USD.inc_by(embedding_cost);

        metrics::EMBEDDING_OVERHEAD_MICRO_USD_TOTAL
            .with_label_values(&[chat_model, operation])
            .inc_by(embedding_cost);

        metrics::REQUEST_COST_MICRO_USD_TOTAL
            .with_label_values(&[chat_model, COST_TYPE_EMBEDDING])
            .inc_by(embedding_cost);

        if embedding_cost == 0 && embedding_prompt_tokens > 0 {
            tracing::debug!(
                model = %chat_model,
                operation = operation,
                embedding_prompt_tokens = embedding_prompt_tokens,
                "no configured embedding_price; embedding overhead metrics recorded zero cost"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cache::exact::ExactCache,
        config::{EmbeddingPrice, ModelPrice},
        core::normalize::normalize_chat_request,
        embeddings::provider::EmbeddingUsage,
        semantic::semantic_cache::{SemanticCache, SemanticLookupHit},
        types::openai::{
            ChatCompletionRequest, ChatCompletionResponse, ChatMessage, Choice, Usage,
        },
        upstream::llm::LlmUpstream,
    };
    use async_trait::async_trait;
    use serde_json::{json, Map, Value};
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    #[derive(Default)]
    struct ExactCacheState {
        entries: HashMap<String, String>,
        get_calls: usize,
        set_calls: usize,
    }

    struct FakeExactCache {
        state: Arc<Mutex<ExactCacheState>>,
    }

    impl FakeExactCache {
        fn new() -> Self {
            Self {
                state: Arc::new(Mutex::new(ExactCacheState::default())),
            }
        }

        fn with_entry(key: String, value: String) -> Self {
            let mut entries = HashMap::new();
            entries.insert(key, value);

            Self {
                state: Arc::new(Mutex::new(ExactCacheState {
                    entries,
                    get_calls: 0,
                    set_calls: 0,
                })),
            }
        }

        fn state(&self) -> Arc<Mutex<ExactCacheState>> {
            Arc::clone(&self.state)
        }
    }

    #[async_trait]
    impl ExactCache for FakeExactCache {
        async fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
            let mut state = self.state.lock().unwrap();
            state.get_calls += 1;
            Ok(state.entries.get(key).cloned())
        }

        async fn set(&self, key: &str, value: String) -> anyhow::Result<()> {
            let mut state = self.state.lock().unwrap();
            state.set_calls += 1;
            state.entries.insert(key.to_string(), value);
            Ok(())
        }
    }

    #[derive(Default)]
    struct SemanticCacheState {
        lookup_result: Option<SemanticLookupHit>,
        lookup_error: Option<String>,
        store_error: Option<String>,
        lookup_calls: usize,
        store_calls: usize,
        last_store_model: Option<String>,
        last_store_prompt: Option<String>,
        last_store_response: Option<ChatCompletionResponse>,
    }

    struct FakeSemanticCache {
        state: Arc<Mutex<SemanticCacheState>>,
    }

    impl FakeSemanticCache {
        fn new() -> Self {
            Self {
                state: Arc::new(Mutex::new(SemanticCacheState::default())),
            }
        }

        fn with_lookup_result(hit: SemanticLookupHit) -> Self {
            Self {
                state: Arc::new(Mutex::new(SemanticCacheState {
                    lookup_result: Some(hit),
                    ..Default::default()
                })),
            }
        }

        fn state(&self) -> Arc<Mutex<SemanticCacheState>> {
            Arc::clone(&self.state)
        }

        fn with_lookup_error(message: &str) -> Self {
            Self {
                state: Arc::new(Mutex::new(SemanticCacheState {
                    lookup_error: Some(message.to_string()),
                    ..Default::default()
                })),
            }
        }

        fn with_store_error(message: &str) -> Self {
            Self {
                state: Arc::new(Mutex::new(SemanticCacheState {
                    store_error: Some(message.to_string()),
                    ..Default::default()
                })),
            }
        }
    }

    #[async_trait]
    impl SemanticCache for FakeSemanticCache {
        async fn lookup(
            &self,
            _model: &str,
            _normalized_prompt: &str,
        ) -> anyhow::Result<Option<SemanticLookupHit>> {
            let mut state = self.state.lock().unwrap();
            state.lookup_calls += 1;

            if let Some(err) = &state.lookup_error {
                anyhow::bail!("{}", err);
            }

            Ok(state.lookup_result.clone())
        }

        async fn store(
            &self,
            model: &str,
            normalized_prompt: &str,
            response: &ChatCompletionResponse,
        ) -> anyhow::Result<Option<EmbeddingUsage>> {
            let mut state = self.state.lock().unwrap();
            state.store_calls += 1;

            if let Some(err) = &state.store_error {
                anyhow::bail!("{}", err);
            }

            state.last_store_model = Some(model.to_string());
            state.last_store_prompt = Some(normalized_prompt.to_string());
            state.last_store_response = Some(response.clone());

            Ok(None)
        }
    }

    #[derive(Default)]
    struct UpstreamState {
        call_count: usize,
        last_request: Option<ChatCompletionRequest>,
    }

    struct FakeUpstream {
        response: ChatCompletionResponse,
        state: Arc<Mutex<UpstreamState>>,
    }

    impl FakeUpstream {
        fn new(response: ChatCompletionResponse) -> Self {
            Self {
                response,
                state: Arc::new(Mutex::new(UpstreamState::default())),
            }
        }

        fn state(&self) -> Arc<Mutex<UpstreamState>> {
            Arc::clone(&self.state)
        }
    }

    #[async_trait]
    impl LlmUpstream for FakeUpstream {
        async fn chat_completion(
            &self,
            req: &ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, AppError> {
            let mut state = self.state.lock().unwrap();
            state.call_count += 1;
            state.last_request = Some(req.clone());
            Ok(self.response.clone())
        }
    }

    fn request() -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "gpt-4o-mini-2024-07-18".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: json!("How do I reset my password?"),
                name: None,
            }],
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        }
    }

    fn response_with_usage(
        id: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) -> ChatCompletionResponse {
        ChatCompletionResponse {
            id: id.to_string(),
            object: "chat.completion".to_string(),
            created: 1_711_111_111,
            model: "gpt-4o-mini-2024-07-18".to_string(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: json!("Use the reset link on the login page."),
                    name: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(Usage {
                prompt_tokens,
                completion_tokens,
                total_tokens: prompt_tokens + completion_tokens,
            }),
            extra: Map::new(),
        }
    }

    fn response_with_content(content: &str) -> ChatCompletionResponse {
        let mut response = response_with_usage("upstream-response", 1000, 500);
        response.choices[0].message.content = json!(content);
        response
    }

    fn model_prices() -> HashMap<String, ModelPrice> {
        let mut prices = HashMap::new();
        prices.insert(
            "gpt-4o-mini-2024-07-18".to_string(),
            ModelPrice {
                input_usd_per_1m_tokens: 0.15,
                output_usd_per_1m_tokens: 0.60,
            },
        );
        prices
    }

    fn build_service(
        exact_cache: Arc<dyn ExactCache>,
        semantic_cache: Arc<dyn SemanticCache>,
        upstream: Arc<dyn LlmUpstream>,
        semantic_cache_enabled: bool,
    ) -> ChatService {
        let settings = ChatServiceSettings {
            semantic_cache_enabled,
            exact_cache_enabled: true,
            exact_cache_fail_open: true,
            exact_cache_store_enabled: true,
            semantic_cache_store_enabled: true,
            semantic_cache_fail_open: true,
            max_prompt_chars: Some(200_000),
        };

        ChatService::new(
            exact_cache,
            semantic_cache,
            upstream,
            settings,
            model_prices(),
            Some(EmbeddingPrice {
                usd_per_1m_tokens: 0.020,
            }),
        )
    }

    fn exact_key_for(service: &ChatService, req: &ChatCompletionRequest) -> String {
        let normalized = normalize_chat_request(req).unwrap();
        service.exact_cache_key(&normalized)
    }

    #[tokio::test]
    async fn exact_hit_returns_cached_response_and_skips_upstream() {
        let req = request();
        let cached = response_with_usage("exact-hit", 1000, 500);

        let probe_service = build_service(
            Arc::new(FakeExactCache::new()),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(FakeUpstream::new(response_with_usage("unused", 1, 1))),
            true,
        );
        let key = exact_key_for(&probe_service, &req);

        let exact_cache = FakeExactCache::with_entry(key, serde_json::to_string(&cached).unwrap());
        let exact_state = exact_cache.state();

        let semantic_cache = FakeSemanticCache::new();
        let semantic_state = semantic_cache.state();

        let upstream = FakeUpstream::new(response_with_usage("upstream", 10, 5));
        let upstream_state = upstream.state();

        let service = build_service(
            Arc::new(exact_cache),
            Arc::new(semantic_cache),
            Arc::new(upstream),
            true,
        );

        let result = service.handle(req).await.unwrap();

        assert_eq!(result.id, "exact-hit");
        assert_eq!(upstream_state.lock().unwrap().call_count, 0);
        assert_eq!(semantic_state.lock().unwrap().lookup_calls, 0);

        let exact = exact_state.lock().unwrap();
        assert_eq!(exact.get_calls, 1);
        assert_eq!(exact.set_calls, 0);
    }

    #[tokio::test]
    async fn semantic_hit_returns_semantic_response_warms_exact_cache_and_skips_upstream() {
        let req = request();
        let semantic_response = response_with_usage("semantic-hit", 1000, 500);

        let semantic_cache = FakeSemanticCache::with_lookup_result(SemanticLookupHit {
            response: semantic_response.clone(),
            embedding_usage: Some(EmbeddingUsage {
                prompt_tokens: 1000,
                total_tokens: 1000,
            }),
        });
        let semantic_state = semantic_cache.state();

        let exact_cache = FakeExactCache::new();
        let exact_state = exact_cache.state();

        let upstream = FakeUpstream::new(response_with_usage("upstream", 10, 5));
        let upstream_state = upstream.state();

        let service = build_service(
            Arc::new(exact_cache),
            Arc::new(semantic_cache),
            Arc::new(upstream),
            true,
        );

        let result = service.handle(req).await.unwrap();

        assert_eq!(result.id, "semantic-hit");
        assert_eq!(upstream_state.lock().unwrap().call_count, 0);
        assert_eq!(semantic_state.lock().unwrap().lookup_calls, 1);

        let exact = exact_state.lock().unwrap();
        assert_eq!(exact.get_calls, 1);
        assert_eq!(exact.set_calls, 1);
        assert_eq!(exact.entries.len(), 1);
    }

    #[tokio::test]
    async fn miss_calls_upstream_and_stores_in_exact_and_semantic_cache() {
        let req = request();
        let upstream_response = response_with_usage("upstream-response", 1000, 500);

        let exact_cache = FakeExactCache::new();
        let exact_state = exact_cache.state();

        let semantic_cache = FakeSemanticCache::new();
        let semantic_state = semantic_cache.state();

        let upstream = FakeUpstream::new(upstream_response.clone());
        let upstream_state = upstream.state();

        let service = build_service(
            Arc::new(exact_cache),
            Arc::new(semantic_cache),
            Arc::new(upstream),
            true,
        );

        let result = service.handle(req.clone()).await.unwrap();

        assert_eq!(result.id, "upstream-response");

        let upstream = upstream_state.lock().unwrap();
        assert_eq!(upstream.call_count, 1);
        assert_eq!(upstream.last_request.as_ref().unwrap().model, req.model);
        drop(upstream);

        let exact = exact_state.lock().unwrap();
        assert_eq!(exact.get_calls, 1);
        assert_eq!(exact.set_calls, 1);
        assert_eq!(exact.entries.len(), 1);
        drop(exact);

        let semantic = semantic_state.lock().unwrap();
        assert_eq!(semantic.lookup_calls, 1);
        assert_eq!(semantic.store_calls, 1);
        assert_eq!(
            semantic.last_store_response.as_ref().unwrap().id,
            "upstream-response"
        );
    }

    #[tokio::test]
    async fn stream_requests_bypass_exact_and_semantic_cache() {
        let mut req = request();
        req.stream = Some(true);

        let exact_cache = FakeExactCache::new();
        let exact_state = exact_cache.state();

        let semantic_cache = FakeSemanticCache::new();
        let semantic_state = semantic_cache.state();

        let upstream = FakeUpstream::new(response_with_usage("stream-upstream", 1000, 500));
        let upstream_state = upstream.state();

        let service = build_service(
            Arc::new(exact_cache),
            Arc::new(semantic_cache),
            Arc::new(upstream),
            true,
        );

        let result = service.handle(req).await.unwrap();

        assert_eq!(result.id, "stream-upstream");
        assert_eq!(upstream_state.lock().unwrap().call_count, 1);

        let exact = exact_state.lock().unwrap();
        assert_eq!(exact.get_calls, 0);
        assert_eq!(exact.set_calls, 0);
        drop(exact);

        let semantic = semantic_state.lock().unwrap();
        assert_eq!(semantic.lookup_calls, 0);
        assert_eq!(semantic.store_calls, 0);
    }

    #[tokio::test]
    async fn tools_requests_skip_semantic_lookup_and_store() {
        let mut req = request();
        req.extra.insert(
            "tools".to_string(),
            Value::Array(vec![json!({"type": "function"})]),
        );

        let exact_cache = FakeExactCache::new();
        let exact_state = exact_cache.state();

        let semantic_cache = FakeSemanticCache::new();
        let semantic_state = semantic_cache.state();

        let upstream = FakeUpstream::new(response_with_usage("tools-upstream", 1000, 500));
        let upstream_state = upstream.state();

        let service = build_service(
            Arc::new(exact_cache),
            Arc::new(semantic_cache),
            Arc::new(upstream),
            true,
        );

        let result = service.handle(req).await.unwrap();

        assert_eq!(result.id, "tools-upstream");
        assert_eq!(upstream_state.lock().unwrap().call_count, 1);

        let exact = exact_state.lock().unwrap();
        assert_eq!(exact.get_calls, 1);
        assert_eq!(exact.set_calls, 1);
        drop(exact);

        let semantic = semantic_state.lock().unwrap();
        assert_eq!(semantic.lookup_calls, 0);
        assert_eq!(semantic.store_calls, 0);
    }

    #[tokio::test]
    async fn semantic_lookup_error_fail_open_continues_upstream() {
        let exact_cache = Arc::new(FakeExactCache::new());
        let semantic_cache = Arc::new(FakeSemanticCache::with_lookup_error(
            "embedding provider unavailable",
        ));
        let upstream = Arc::new(FakeUpstream::new(response_with_content(
            "upstream response",
        )));

        let settings = ChatServiceSettings {
            semantic_cache_enabled: true,
            exact_cache_enabled: true,
            exact_cache_fail_open: true,
            exact_cache_store_enabled: true,
            semantic_cache_store_enabled: true,
            semantic_cache_fail_open: true,
            max_prompt_chars: Some(200_000),
        };

        let service = ChatService::new(
            exact_cache,
            semantic_cache,
            upstream,
            settings,
            model_prices(),
            Some(EmbeddingPrice {
                usd_per_1m_tokens: 0.020,
            }),
        );

        let response = service.handle(request()).await.unwrap();

        assert_eq!(
            response.choices[0].message.content,
            json!("upstream response")
        );
    }

    #[tokio::test]
    async fn semantic_lookup_error_fail_closed_returns_error() {
        let exact_cache = Arc::new(FakeExactCache::new());
        let semantic_cache = Arc::new(FakeSemanticCache::with_lookup_error(
            "embedding provider unavailable",
        ));
        let upstream = Arc::new(FakeUpstream::new(response_with_content(
            "upstream response",
        )));

        let settings = ChatServiceSettings {
            semantic_cache_enabled: true,
            exact_cache_enabled: true,
            exact_cache_fail_open: true,
            exact_cache_store_enabled: true,
            semantic_cache_store_enabled: true,
            semantic_cache_fail_open: false,
            max_prompt_chars: Some(200_000),
        };

        let service = ChatService::new(
            exact_cache,
            semantic_cache,
            upstream,
            settings,
            model_prices(),
            Some(EmbeddingPrice {
                usd_per_1m_tokens: 0.020,
            }),
        );

        let err = service.handle(request()).await.unwrap_err();
        let msg = err.to_string();

        assert!(msg.contains("semantic lookup failed"));
        assert!(msg.contains("semantic_cache_fail_open=false"));
    }

    #[tokio::test]
    async fn semantic_store_error_fail_open_does_not_fail_response() {
        let exact_cache = Arc::new(FakeExactCache::new());
        let semantic_cache = Arc::new(FakeSemanticCache::with_store_error(
            "embedding provider unavailable during store",
        ));
        let upstream = Arc::new(FakeUpstream::new(response_with_content(
            "upstream response",
        )));

        let settings = ChatServiceSettings {
            semantic_cache_enabled: true,
            exact_cache_enabled: true,
            exact_cache_fail_open: true,
            exact_cache_store_enabled: true,
            semantic_cache_store_enabled: true,
            semantic_cache_fail_open: true,
            max_prompt_chars: Some(200_000),
        };

        let service = ChatService::new(
            exact_cache,
            semantic_cache,
            upstream,
            settings,
            model_prices(),
            Some(EmbeddingPrice {
                usd_per_1m_tokens: 0.020,
            }),
        );

        let response = service.handle(request()).await.unwrap();

        assert_eq!(
            response.choices[0].message.content,
            json!("upstream response")
        );
    }

    #[tokio::test]
    async fn semantic_store_error_fail_closed_returns_error() {
        let exact_cache = Arc::new(FakeExactCache::new());
        let semantic_cache = Arc::new(FakeSemanticCache::with_store_error(
            "embedding provider unavailable during store",
        ));
        let upstream = Arc::new(FakeUpstream::new(response_with_content(
            "upstream response",
        )));

        let settings = ChatServiceSettings {
            semantic_cache_enabled: true,
            exact_cache_enabled: true,
            exact_cache_fail_open: true,
            exact_cache_store_enabled: true,
            semantic_cache_store_enabled: true,
            semantic_cache_fail_open: false,
            max_prompt_chars: Some(200_000),
        };

        let service = ChatService::new(
            exact_cache,
            semantic_cache,
            upstream,
            settings,
            model_prices(),
            Some(EmbeddingPrice {
                usd_per_1m_tokens: 0.020,
            }),
        );

        let err = service.handle(request()).await.unwrap_err();
        let msg = err.to_string();

        assert!(msg.contains("semantic store failed"));
        assert!(msg.contains("semantic_cache_fail_open=false"));
    }
}
