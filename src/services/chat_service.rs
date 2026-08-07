use std::{collections::HashMap, sync::Arc, time::Instant};
use tokio::sync::Semaphore;

use crate::{
    app::DependencyState,
    cache::{exact::ExactCache, redis_exact::RedisOperationTimeout},
    config::{EmbeddingPrice, ModelPrice},
    core::{
        hashing::sha256_hex,
        normalize::{normalize_chat_request, semantic_text_from_request},
        pricing::{estimate_embedding_micro_usd, estimate_micro_usd_saved},
    },
    error::{AppError, DependencyKind, FailureClass},
    evidence::{
        CacheEvidence, DecisionEvidence, EventCategory, EventOutcome, EvidenceEvent, EvidenceSink,
        EvidenceSource, NoopEvidenceSink, UpstreamEvidence,
    },
    guards::{GuardContext, GuardOrchestrator},
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
    guard_orchestrator: Arc<dyn GuardOrchestrator>,
    exact_cache_enabled: bool,
    exact_cache_fail_open: bool,
    exact_cache_store_enabled: bool,
    semantic_cache_enabled: bool,
    semantic_cache_fail_open: bool,
    semantic_cache_store_enabled: bool,
    max_prompt_chars: Option<usize>,
    model_prices: HashMap<String, ModelPrice>,
    embedding_price: Option<EmbeddingPrice>,
    evidence_sink: Arc<dyn EvidenceSink>,
    upstream_metadata: UpstreamMetadata,
    upstream_limit: Arc<Semaphore>,
    dependencies: DependencyState,
    track_redis_runtime: bool,
    track_qdrant_runtime: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpstreamMetadata {
    pub provider_type: String,
    pub provider_name: String,
}

pub struct ChatServiceDeps {
    pub exact_cache: Arc<dyn ExactCache>,
    pub semantic_cache: Arc<dyn SemanticCache>,
    pub upstream: Arc<dyn LlmUpstream>,
    pub guard_orchestrator: Arc<dyn GuardOrchestrator>,
    pub evidence_sink: Arc<dyn EvidenceSink>,
    pub upstream_metadata: UpstreamMetadata,
    pub dependencies: DependencyState,
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
    pub max_inflight_upstream_requests: usize,
}

fn estimate_prompt_chars(req: &ChatCompletionRequest) -> usize {
    req.messages
        .iter()
        .map(|message| message.content.to_string().chars().count())
        .sum()
}

fn redis_failure_class(error: &anyhow::Error) -> FailureClass {
    if error.downcast_ref::<RedisOperationTimeout>().is_some() {
        FailureClass::Timeout
    } else {
        FailureClass::Unavailable
    }
}

impl ChatService {
    #[cfg(test)]
    pub fn new(
        exact_cache: Arc<dyn ExactCache>,
        semantic_cache: Arc<dyn SemanticCache>,
        upstream: Arc<dyn LlmUpstream>,
        settings: ChatServiceSettings,
        model_prices: HashMap<String, ModelPrice>,
        embedding_price: Option<EmbeddingPrice>,
    ) -> Self {
        Self::new_with_guards(
            exact_cache,
            semantic_cache,
            upstream,
            Arc::new(crate::guards::NoopGuardOrchestrator),
            settings,
            model_prices,
            embedding_price,
        )
    }

    #[allow(dead_code)]
    // Retained for unit tests and compatibility with callers that do not provide a custom evidence sink.
    pub fn new_with_guards(
        exact_cache: Arc<dyn ExactCache>,
        semantic_cache: Arc<dyn SemanticCache>,
        upstream: Arc<dyn LlmUpstream>,
        guard_orchestrator: Arc<dyn GuardOrchestrator>,
        settings: ChatServiceSettings,
        model_prices: HashMap<String, ModelPrice>,
        embedding_price: Option<EmbeddingPrice>,
    ) -> Self {
        Self::new_with_guards_and_evidence(
            ChatServiceDeps {
                exact_cache,
                semantic_cache,
                upstream,
                guard_orchestrator,
                evidence_sink: Arc::new(NoopEvidenceSink),
                upstream_metadata: UpstreamMetadata {
                    provider_type: "test".to_string(),
                    provider_name: "test".to_string(),
                },
                dependencies: DependencyState::new(true, true, true),
            },
            settings,
            model_prices,
            embedding_price,
        )
    }

    pub fn new_with_guards_and_evidence(
        deps: ChatServiceDeps,
        settings: ChatServiceSettings,
        model_prices: HashMap<String, ModelPrice>,
        embedding_price: Option<EmbeddingPrice>,
    ) -> Self {
        let dependencies = deps.dependencies;
        let track_redis_runtime = dependencies
            .redis_available
            .load(std::sync::atomic::Ordering::Relaxed);
        let track_qdrant_runtime = dependencies
            .qdrant_available
            .load(std::sync::atomic::Ordering::Relaxed);

        Self {
            exact_cache: deps.exact_cache,
            semantic_cache: deps.semantic_cache,
            upstream: deps.upstream,
            guard_orchestrator: deps.guard_orchestrator,

            exact_cache_enabled: settings.exact_cache_enabled,
            exact_cache_fail_open: settings.exact_cache_fail_open,
            exact_cache_store_enabled: settings.exact_cache_store_enabled,

            semantic_cache_enabled: settings.semantic_cache_enabled,
            semantic_cache_fail_open: settings.semantic_cache_fail_open,
            semantic_cache_store_enabled: settings.semantic_cache_store_enabled,

            max_prompt_chars: settings.max_prompt_chars,
            model_prices,
            embedding_price,
            evidence_sink: deps.evidence_sink,
            upstream_metadata: deps.upstream_metadata,
            dependencies,
            track_redis_runtime,
            track_qdrant_runtime,
            upstream_limit: Arc::new(Semaphore::new(settings.max_inflight_upstream_requests)),
        }
    }

    fn set_redis_available(&self, available: bool) {
        if self.track_redis_runtime {
            self.dependencies
                .redis_available
                .store(available, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn set_qdrant_available(&self, available: bool) {
        if self.track_qdrant_runtime {
            self.dependencies
                .qdrant_available
                .store(available, std::sync::atomic::Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    pub async fn handle(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, AppError> {
        self.handle_with_evidence(req, CacheControl::default(), uuid::Uuid::new_v4())
            .await
    }

    pub async fn handle_with_evidence(
        &self,
        req: ChatCompletionRequest,
        cache_control: CacheControl,
        trace_id: uuid::Uuid,
    ) -> Result<ChatCompletionResponse, AppError> {
        self.validate(&req)?;
        self.emit(EvidenceEvent::new(
            trace_id,
            EvidenceSource::AiFirewall,
            EventCategory::Request,
            "request.received",
            EventOutcome::Started,
        ))
        .await;

        let result = self
            .handle_after_request_received(req, cache_control, trace_id)
            .await;

        if let Err(error) = &result {
            self.emit(self.request_failed_event(trace_id, error)).await;
        }

        result
    }

    async fn handle_after_request_received(
        &self,
        req: ChatCompletionRequest,
        cache_control: CacheControl,
        trace_id: uuid::Uuid,
    ) -> Result<ChatCompletionResponse, AppError> {
        if req.stream.unwrap_or(false) {
            tracing::warn!(
                model = %req.normalized_model(),
                "stream=true rejected because AI Firewall does not support streaming responses"
            );
            return Err(AppError::unprocessable(
                "stream=true is not supported by AI Firewall; set stream=false",
            ));
        }

        let guarded = self.guard_orchestrator.before_cache(req, trace_id).await?;
        let req = guarded.request;
        let guard_context = guarded.context;
        let cache_control = CacheControl {
            bypass_lookup: cache_control.bypass_lookup || guarded.cache_control.bypass_lookup,
            bypass_store: cache_control.bypass_store || guarded.cache_control.bypass_store,
        };

        let normalized = normalize_chat_request(&req)
            .map_err(|e| AppError::bad_request(format!("normalize failed: {e}")))?;
        let semantic_text = semantic_text_from_request(&req);
        let privacy_placeholder_signature = guard_context.privacy_placeholder_signature.as_deref();

        let exact_key_hash = sha256_hex(&normalized);
        let exact_key = format!("chatcmpl:v1:{exact_key_hash}");

        if self.exact_cache_enabled && !cache_control.bypass_lookup {
            match self.exact_cache.get(&exact_key).await {
                Ok(Some(raw)) => {
                    self.set_redis_available(true);
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

                    let mut event = EvidenceEvent::new(
                        trace_id,
                        EvidenceSource::AiFirewall,
                        EventCategory::Cache,
                        "cache.exact.lookup",
                        EventOutcome::Hit,
                    );
                    event.cache = Some(CacheEvidence {
                        cache_type: "exact".into(),
                        operation: "lookup".into(),
                        outcome: EventOutcome::Hit,
                        cache_key_hash: Some(exact_key_hash.clone()),
                        record_id: None,
                        similarity_score: None,
                        threshold: None,
                        upstream_called: false,
                    });
                    self.emit(event).await;

                    let response = self
                        .finalize_guarded_response(&guard_context, hit, trace_id)
                        .await?;
                    self.emit(self.request_completed_event(trace_id, "exact_cache"))
                        .await;
                    return Ok(response);
                }
                Ok(None) => {
                    self.set_redis_available(true);
                    let mut event = EvidenceEvent::new(
                        trace_id,
                        EvidenceSource::AiFirewall,
                        EventCategory::Cache,
                        "cache.exact.lookup",
                        EventOutcome::Miss,
                    );
                    event.cache = Some(CacheEvidence {
                        cache_type: "exact".into(),
                        operation: "lookup".into(),
                        outcome: EventOutcome::Miss,
                        cache_key_hash: Some(exact_key_hash.clone()),
                        record_id: None,
                        similarity_score: None,
                        threshold: None,
                        upstream_called: false,
                    });
                    self.emit(event).await;
                }
                Err(e) if self.exact_cache_fail_open => {
                    self.set_redis_available(false);
                    tracing::warn!(
                        model = %req.normalized_model(),
                        error = %e,
                        "exact cache lookup failed; exact_cache_fail_open=true so request continues"
                    );
                }
                Err(e) => {
                    self.set_redis_available(false);
                    return Err(AppError::dependency_failure(
                        DependencyKind::Redis,
                        redis_failure_class(&e),
                        format!("exact cache get failed: {e}"),
                    ));
                }
            }
        } else if cache_control.bypass_lookup {
            tracing::debug!(
                model = %req.normalized_model(),
                "cache lookup bypass requested"
            );
            self.emit(EvidenceEvent::new(
                trace_id,
                EvidenceSource::AiFirewall,
                EventCategory::Cache,
                "cache.lookup.bypassed",
                EventOutcome::Bypassed,
            ))
            .await;
        }

        if self.semantic_cache_enabled
            && !cache_control.bypass_lookup
            && self.semantic_eligible(&req)
        {
            match self
                .semantic_cache
                .lookup(
                    req.normalized_model(),
                    &semantic_text,
                    privacy_placeholder_signature,
                )
                .await
            {
                Ok(Some(hit)) => {
                    self.set_qdrant_available(true);
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
                    let mut event = EvidenceEvent::new(
                        trace_id,
                        EvidenceSource::AiFirewall,
                        EventCategory::Cache,
                        "cache.semantic.lookup",
                        EventOutcome::Hit,
                    );
                    event.cache = Some(CacheEvidence {
                        cache_type: "semantic".into(),
                        operation: "lookup".into(),
                        outcome: EventOutcome::Hit,
                        cache_key_hash: None,
                        record_id: None,
                        similarity_score: None,
                        threshold: None,
                        upstream_called: false,
                    });
                    self.emit(event).await;

                    if self.exact_cache_enabled
                        && self.exact_cache_store_enabled
                        && !cache_control.bypass_store
                    {
                        if let Ok(raw) = serde_json::to_string(&hit.response) {
                            match self.exact_cache.set(&exact_key, raw).await {
                                Ok(()) => {
                                    self.set_redis_available(true);
                                }
                                Err(e) if self.exact_cache_fail_open => {
                                    self.set_redis_available(false);
                                    tracing::debug!(
                                        "failed to warm exact cache from semantic hit: {e}"
                                    );
                                }
                                Err(e) => {
                                    self.set_redis_available(false);
                                    return Err(AppError::dependency_failure(
                                        DependencyKind::Redis,
                                        redis_failure_class(&e),
                                        format!("exact cache set failed while warming semantic hit: {e}"),
                                    ));
                                }
                            }
                        } else {
                            tracing::debug!(
                                "failed to serialize semantic-hit response for exact cache warming"
                            );
                        }
                    }
                    let response = self
                        .finalize_guarded_response(&guard_context, hit.response, trace_id)
                        .await?;
                    self.emit(self.request_completed_event(trace_id, "semantic_cache"))
                        .await;
                    return Ok(response);
                }

                Ok(None) => {
                    self.set_qdrant_available(true);
                    let mut event = EvidenceEvent::new(
                        trace_id,
                        EvidenceSource::AiFirewall,
                        EventCategory::Cache,
                        "cache.semantic.lookup",
                        EventOutcome::Miss,
                    );
                    event.cache = Some(CacheEvidence {
                        cache_type: "semantic".into(),
                        operation: "lookup".into(),
                        outcome: EventOutcome::Miss,
                        cache_key_hash: None,
                        record_id: None,
                        similarity_score: None,
                        threshold: None,
                        upstream_called: false,
                    });
                    self.emit(event).await;
                }

                Err(e) if self.semantic_cache_fail_open => {
                    self.set_qdrant_available(false);
                    self.record_semantic_skip("lookup_error");

                    tracing::warn!(
                        model = %req.normalized_model(),
                        error = %e,
                        "semantic lookup failed; skipping semantic cache and continuing upstream"
                    );
                }

                Err(e) => {
                    self.set_qdrant_available(false);
                    return Err(AppError::dependency_failure(
                        DependencyKind::Qdrant,
                        FailureClass::Unavailable,
                        format!("semantic lookup failed and semantic_cache_fail_open=false: {e}"),
                    ));
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
        self.emit(EvidenceEvent::new(
            trace_id,
            EvidenceSource::AiFirewall,
            EventCategory::Cache,
            "cache.lookup.completed",
            EventOutcome::Miss,
        ))
        .await;

        tracing::debug!(
            model = %req.normalized_model(),
            "cache miss; forwarding request upstream"
        );

        let mut upstream_event = EvidenceEvent::new(
            trace_id,
            EvidenceSource::AiFirewall,
            EventCategory::Upstream,
            "upstream.request.sent",
            EventOutcome::Started,
        );
        upstream_event.upstream = Some(UpstreamEvidence {
            provider_type: self.upstream_metadata.provider_type.clone(),
            provider_name: self.upstream_metadata.provider_name.clone(),
            model: req.normalized_model().to_string(),
            endpoint_class: Some("chat_completions".into()),
            response_status: None,
            latency_ms: None,
        });
        self.emit(upstream_event).await;

        let _upstream_permit = self.upstream_limit.try_acquire().map_err(|_| {
            metrics::observe_backpressure_rejection("upstream");
            AppError::backpressure("upstream", "upstream concurrency limit reached")
        })?;
        let upstream_started = Instant::now();
        let response = match self.upstream.chat_completion(&req).await {
            Ok(response) => {
                self.dependencies
                    .upstream_available
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                response
            }
            Err(error) => {
                self.dependencies
                    .upstream_available
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                let latency_ms = upstream_started.elapsed().as_millis() as u64;
                let reason_code = error.evidence_reason_code();

                let mut upstream_failed = EvidenceEvent::new(
                    trace_id,
                    EvidenceSource::AiFirewall,
                    EventCategory::Upstream,
                    "upstream.request.failed",
                    EventOutcome::Failed,
                );
                upstream_failed.upstream = Some(UpstreamEvidence {
                    provider_type: self.upstream_metadata.provider_type.clone(),
                    provider_name: self.upstream_metadata.provider_name.clone(),
                    model: req.normalized_model().to_string(),
                    endpoint_class: Some("chat_completions".into()),
                    response_status: None,
                    latency_ms: Some(latency_ms),
                });
                upstream_failed.decision = Some(DecisionEvidence {
                    action: "fail_request".into(),
                    reason_code: reason_code.into(),
                    rule_id: None,
                    severity: None,
                });
                self.emit(upstream_failed).await;

                return Err(error);
            }
        };

        let latency_ms = upstream_started.elapsed().as_millis() as u64;
        let mut upstream_received = EvidenceEvent::new(
            trace_id,
            EvidenceSource::AiFirewall,
            EventCategory::Upstream,
            "upstream.response.received",
            EventOutcome::Completed,
        );
        upstream_received.upstream = Some(UpstreamEvidence {
            provider_type: self.upstream_metadata.provider_type.clone(),
            provider_name: self.upstream_metadata.provider_name.clone(),
            model: response.model.clone(),
            endpoint_class: Some("chat_completions".into()),
            response_status: Some(200),
            latency_ms: Some(latency_ms),
        });
        self.emit(upstream_received).await;

        self.record_upstream_model_cost(&response);

        let raw = serde_json::to_string(&response)
            .map_err(|e| AppError::internal(format!("response encode failed: {e}")))?;

        if self.exact_cache_enabled && self.exact_cache_store_enabled && !cache_control.bypass_store
        {
            match self.exact_cache.set(&exact_key, raw).await {
                Ok(()) => {
                    self.set_redis_available(true);
                }
                Err(e) if self.exact_cache_fail_open => {
                    self.set_redis_available(false);
                    tracing::warn!(
                        model = %req.normalized_model(),
                        error = %e,
                        "exact cache store failed; exact_cache_fail_open=true so response is returned"
                    );
                }
                Err(e) => {
                    self.set_redis_available(false);
                    return Err(AppError::dependency_failure(
                        DependencyKind::Redis,
                        redis_failure_class(&e),
                        format!("exact cache set failed: {e}"),
                    ));
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
                .store(
                    req.normalized_model(),
                    &semantic_text,
                    &response,
                    privacy_placeholder_signature,
                )
                .await
            {
                Ok(embedding_usage) => {
                    self.set_qdrant_available(true);
                    if let Some(usage) = embedding_usage {
                        self.record_embedding_overhead(
                            req.normalized_model(),
                            EMBEDDING_OPERATION_STORE,
                            usage.prompt_tokens,
                        );
                    }
                }

                Err(e) if self.semantic_cache_fail_open => {
                    self.set_qdrant_available(false);
                    self.record_semantic_skip("store_error");

                    tracing::warn!(
                        model = %req.normalized_model(),
                        error = %e,
                        "semantic store failed; response returned without semantic cache write"
                    );
                }

                Err(e) => {
                    self.set_qdrant_available(false);
                    self.record_semantic_skip("store_error");

                    return Err(AppError::dependency_failure(
                        DependencyKind::Qdrant,
                        FailureClass::Unavailable,
                        format!("semantic store failed and semantic_cache_fail_open=false: {e}"),
                    ));
                }
            }
        }

        let response = self
            .finalize_guarded_response(&guard_context, response, trace_id)
            .await?;
        self.emit(self.request_completed_event(trace_id, "upstream"))
            .await;
        Ok(response)
    }

    async fn finalize_guarded_response(
        &self,
        guard_context: &GuardContext,
        response: ChatCompletionResponse,
        trace_id: uuid::Uuid,
    ) -> Result<ChatCompletionResponse, AppError> {
        // Response path guard order is intentional:
        // 1. Security Guard scans the current assistant response.
        //    If Privacy Guard is enabled, this response is still anonymized.
        //    If Privacy Guard is disabled, this response is the original assistant response.
        // 2. If Security Guard blocks the response, return the security error and do not restore.
        // 3. Privacy Guard restores only responses that passed the response security scan.
        let response = self
            .guard_orchestrator
            .before_response_restore(guard_context, response, trace_id)
            .await?;

        self.guard_orchestrator
            .restore_response(guard_context, response, trace_id)
            .await
    }

    fn request_failed_event(&self, trace_id: uuid::Uuid, error: &AppError) -> EvidenceEvent {
        let mut event = EvidenceEvent::new(
            trace_id,
            EvidenceSource::AiFirewall,
            EventCategory::Request,
            "request.failed",
            EventOutcome::Failed,
        );
        event.decision = Some(DecisionEvidence {
            action: if error.is_security_block() {
                "block".into()
            } else {
                "fail_request".into()
            },
            reason_code: error.evidence_reason_code().into(),
            rule_id: error.security_block_rule_id().map(str::to_string),
            severity: None,
        });
        event.attributes.insert(
            "error_class".to_string(),
            serde_json::Value::String(error.metrics_class().to_string()),
        );
        event
    }

    fn request_completed_event(
        &self,
        trace_id: uuid::Uuid,
        delivery_path: &'static str,
    ) -> EvidenceEvent {
        let mut event = EvidenceEvent::new(
            trace_id,
            EvidenceSource::AiFirewall,
            EventCategory::Request,
            "request.completed",
            EventOutcome::Completed,
        );
        event.attributes.insert(
            "delivery_path".to_string(),
            serde_json::Value::String(delivery_path.to_string()),
        );
        event
    }

    async fn emit(&self, event: EvidenceEvent) {
        if let Err(error) = self.evidence_sink.emit(event).await {
            tracing::warn!(error = %error, "failed to emit VCAL evidence event");
        }
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
        app::DependencyState,
        cache::exact::ExactCache,
        config::{EmbeddingPrice, ModelPrice},
        core::normalize::normalize_chat_request,
        embeddings::provider::EmbeddingUsage,
        guards::{GuardContext, GuardOrchestrator, GuardedRequest},
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
        last_lookup_privacy_placeholder_signature: Option<String>,
        last_store_privacy_placeholder_signature: Option<String>,
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
            privacy_placeholder_signature: Option<&str>,
        ) -> anyhow::Result<Option<SemanticLookupHit>> {
            let mut state = self.state.lock().unwrap();
            state.lookup_calls += 1;
            state.last_lookup_privacy_placeholder_signature =
                privacy_placeholder_signature.map(ToOwned::to_owned);

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
            privacy_placeholder_signature: Option<&str>,
        ) -> anyhow::Result<Option<EmbeddingUsage>> {
            let mut state = self.state.lock().unwrap();
            state.store_calls += 1;
            state.last_store_privacy_placeholder_signature =
                privacy_placeholder_signature.map(ToOwned::to_owned);

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

    struct SignatureGuard(&'static str);

    #[async_trait]
    impl GuardOrchestrator for SignatureGuard {
        async fn before_cache(
            &self,
            request: ChatCompletionRequest,
            _trace_id: uuid::Uuid,
        ) -> Result<GuardedRequest, AppError> {
            Ok(GuardedRequest {
                request,
                context: GuardContext {
                    privacy_mapping_id: Some("mapping-test".to_string()),
                    privacy_tenant_id: None,
                    privacy_placeholder_signature: Some(self.0.to_string()),
                    ..GuardContext::default()
                },
                cache_control: CacheControl::default(),
            })
        }

        async fn restore_response(
            &self,
            _context: &GuardContext,
            response: ChatCompletionResponse,
            _trace_id: uuid::Uuid,
        ) -> Result<ChatCompletionResponse, AppError> {
            Ok(response)
        }
    }

    fn request() -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "gpt-4o-mini-2024-07-18".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: json!("How do I reset my password?"),
                name: None,
                extra: Map::new(),
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
                    extra: Map::new(),
                },
                finish_reason: Some("stop".to_string()),
                extra: serde_json::Map::new(),
            }],
            usage: Some(Usage {
                prompt_tokens,
                completion_tokens,
                total_tokens: prompt_tokens + completion_tokens,
                extra: serde_json::Map::new(),
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
            max_inflight_upstream_requests: 500,
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

    fn exact_key_for(req: &ChatCompletionRequest) -> String {
        let normalized = normalize_chat_request(req).unwrap();
        format!("chatcmpl:v1:{}", sha256_hex(&normalized))
    }

    struct FailingExactCache;

    #[async_trait]
    impl ExactCache for FailingExactCache {
        async fn get(&self, _key: &str) -> anyhow::Result<Option<String>> {
            anyhow::bail!("simulated Redis outage")
        }

        async fn set(&self, _key: &str, _value: String) -> anyhow::Result<()> {
            anyhow::bail!("simulated Redis outage")
        }
    }

    struct FailingUpstream;

    #[async_trait]
    impl LlmUpstream for FailingUpstream {
        async fn chat_completion(
            &self,
            _req: &ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, AppError> {
            Err(AppError::upstream_kind(
                crate::upstream::llm::UpstreamErrorKind::Timeout,
                "simulated upstream timeout",
            ))
        }
    }

    #[derive(Default)]
    struct RecordingEvidenceSink {
        events: Arc<Mutex<Vec<crate::evidence::EvidenceEvent>>>,
    }

    #[async_trait]
    impl crate::evidence::EvidenceSink for RecordingEvidenceSink {
        async fn emit(&self, event: crate::evidence::EvidenceEvent) -> anyhow::Result<()> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }
    }

    struct UserMappingGuard;

    #[async_trait]
    impl GuardOrchestrator for UserMappingGuard {
        async fn before_cache(
            &self,
            mut request: ChatCompletionRequest,
            _trace_id: uuid::Uuid,
        ) -> Result<GuardedRequest, AppError> {
            let original = request.messages[0]
                .content
                .as_str()
                .unwrap_or_default()
                .to_string();
            let email = original
                .split_whitespace()
                .find(|part| part.contains('@'))
                .unwrap_or_default()
                .trim_matches(|c: char| c == ',' || c == '.')
                .to_string();
            request.messages[0].content = json!("Email [EMAIL_1]");

            Ok(GuardedRequest {
                request,
                context: GuardContext {
                    privacy_mapping_id: Some("mapping".to_string()),
                    privacy_tenant_id: Some(email),
                    privacy_placeholder_signature: Some("EMAIL:1".to_string()),
                    privacy_modified: true,
                    ..GuardContext::default()
                },
                cache_control: CacheControl::default(),
            })
        }

        async fn restore_response(
            &self,
            context: &GuardContext,
            mut response: ChatCompletionResponse,
            _trace_id: uuid::Uuid,
        ) -> Result<ChatCompletionResponse, AppError> {
            if let Some(email) = context.privacy_tenant_id.as_deref() {
                let content = response.choices[0]
                    .message
                    .content
                    .as_str()
                    .unwrap_or_default()
                    .replace("[EMAIL_1]", email);
                response.choices[0].message.content = json!(content);
            }
            Ok(response)
        }
    }

    #[tokio::test]
    async fn redis_outage_fails_open_and_reaches_upstream_when_configured() {
        let upstream = FakeUpstream::new(response_with_content("upstream ok"));
        let state = upstream.state();
        let service = ChatService::new(
            Arc::new(FailingExactCache),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(upstream),
            ChatServiceSettings {
                semantic_cache_enabled: false,
                exact_cache_enabled: true,
                exact_cache_fail_open: true,
                exact_cache_store_enabled: true,
                semantic_cache_store_enabled: false,
                semantic_cache_fail_open: true,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 500,
            },
            model_prices(),
            None,
        );

        let result = service.handle(request()).await;
        assert!(result.is_ok());
        assert_eq!(state.lock().unwrap().call_count, 1);
    }

    #[tokio::test]
    async fn redis_outage_fails_closed_when_configured() {
        let service = ChatService::new(
            Arc::new(FailingExactCache),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(FakeUpstream::new(response_with_content("unused"))),
            ChatServiceSettings {
                semantic_cache_enabled: false,
                exact_cache_enabled: true,
                exact_cache_fail_open: false,
                exact_cache_store_enabled: true,
                semantic_cache_store_enabled: false,
                semantic_cache_fail_open: true,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 500,
            },
            model_prices(),
            None,
        );

        let error = service
            .handle(request())
            .await
            .expect_err("Redis outage must fail closed");
        assert_eq!(error.metrics_class(), "dependency_failure");
        assert_eq!(
            error.status_code(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(error.message().contains("exact cache get failed"));
    }

    #[tokio::test]
    async fn qdrant_outage_fails_open_and_reaches_upstream_when_configured() {
        let upstream = FakeUpstream::new(response_with_content("upstream ok"));
        let state = upstream.state();
        let service = build_service(
            Arc::new(FakeExactCache::new()),
            Arc::new(FakeSemanticCache::with_lookup_error(
                "simulated Qdrant outage",
            )),
            Arc::new(upstream),
            true,
        );

        let result = service.handle(request()).await;
        assert!(result.is_ok());
        assert_eq!(state.lock().unwrap().call_count, 1);
    }

    #[tokio::test]
    async fn qdrant_outage_fails_closed_when_configured() {
        let service = ChatService::new(
            Arc::new(FakeExactCache::new()),
            Arc::new(FakeSemanticCache::with_lookup_error(
                "simulated Qdrant outage",
            )),
            Arc::new(FakeUpstream::new(response_with_content("unused"))),
            ChatServiceSettings {
                semantic_cache_enabled: true,
                exact_cache_enabled: false,
                exact_cache_fail_open: true,
                exact_cache_store_enabled: false,
                semantic_cache_store_enabled: true,
                semantic_cache_fail_open: false,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 500,
            },
            model_prices(),
            None,
        );

        let error = service
            .handle(request())
            .await
            .expect_err("Qdrant outage must fail closed");
        assert_eq!(error.metrics_class(), "dependency_failure");
        assert_eq!(
            error.status_code(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(error.message().contains("semantic lookup failed"));
    }

    #[tokio::test]
    async fn successful_request_emits_exactly_one_completed_terminal_event() {
        let sink = RecordingEvidenceSink::default();
        let events = Arc::clone(&sink.events);
        let service = ChatService::new_with_guards_and_evidence(
            ChatServiceDeps {
                exact_cache: Arc::new(FakeExactCache::new()),
                semantic_cache: Arc::new(FakeSemanticCache::new()),
                upstream: Arc::new(FakeUpstream::new(response_with_content("ok"))),
                guard_orchestrator: Arc::new(crate::guards::NoopGuardOrchestrator),
                evidence_sink: Arc::new(sink),
                upstream_metadata: UpstreamMetadata {
                    provider_type: "test".into(),
                    provider_name: "test".into(),
                },
                dependencies: DependencyState::new(true, true, true),
            },
            ChatServiceSettings {
                semantic_cache_enabled: false,
                exact_cache_enabled: false,
                exact_cache_fail_open: true,
                exact_cache_store_enabled: false,
                semantic_cache_store_enabled: false,
                semantic_cache_fail_open: true,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 500,
            },
            model_prices(),
            None,
        );

        let trace_id = uuid::Uuid::new_v4();
        service
            .handle_with_evidence(request(), CacheControl::default(), trace_id)
            .await
            .unwrap();

        let events = events.lock().unwrap();
        let names: Vec<_> = events
            .iter()
            .filter(|event| event.trace_id == trace_id)
            .map(|event| event.event_type.as_str())
            .collect();
        assert_eq!(
            names
                .iter()
                .filter(|name| **name == "request.received")
                .count(),
            1
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| **name == "request.completed")
                .count(),
            1
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| **name == "request.failed")
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn upstream_timeout_emits_one_failed_terminal_event() {
        let sink = RecordingEvidenceSink::default();
        let events = Arc::clone(&sink.events);
        let service = ChatService::new_with_guards_and_evidence(
            ChatServiceDeps {
                exact_cache: Arc::new(FakeExactCache::new()),
                semantic_cache: Arc::new(FakeSemanticCache::new()),
                upstream: Arc::new(FailingUpstream),
                guard_orchestrator: Arc::new(crate::guards::NoopGuardOrchestrator),
                evidence_sink: Arc::new(sink),
                upstream_metadata: UpstreamMetadata {
                    provider_type: "test".into(),
                    provider_name: "test".into(),
                },
                dependencies: DependencyState::new(true, true, true),
            },
            ChatServiceSettings {
                semantic_cache_enabled: false,
                exact_cache_enabled: false,
                exact_cache_fail_open: true,
                exact_cache_store_enabled: false,
                semantic_cache_store_enabled: false,
                semantic_cache_fail_open: true,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 500,
            },
            model_prices(),
            None,
        );

        let trace_id = uuid::Uuid::new_v4();
        let error = service
            .handle_with_evidence(request(), CacheControl::default(), trace_id)
            .await
            .expect_err("timeout should fail request");
        assert_eq!(error.metrics_class(), "upstream_timeout");

        let events = events.lock().unwrap();
        let names: Vec<_> = events
            .iter()
            .filter(|event| event.trace_id == trace_id)
            .map(|event| event.event_type.as_str())
            .collect();
        assert_eq!(
            names
                .iter()
                .filter(|name| **name == "request.received")
                .count(),
            1
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| **name == "request.failed")
                .count(),
            1
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| **name == "request.completed")
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn cross_user_exact_cache_restores_only_current_users_pii() {
        let exact = FakeExactCache::new();
        let upstream = FakeUpstream::new(response_with_content("Response for [EMAIL_1]"));
        let upstream_state = upstream.state();
        let service = ChatService::new_with_guards(
            Arc::new(exact),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(upstream),
            Arc::new(UserMappingGuard),
            ChatServiceSettings {
                semantic_cache_enabled: false,
                exact_cache_enabled: true,
                exact_cache_fail_open: true,
                exact_cache_store_enabled: true,
                semantic_cache_store_enabled: false,
                semantic_cache_fail_open: true,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 500,
            },
            model_prices(),
            None,
        );

        let mut alice = request();
        alice.messages[0].content = json!("Email alice@example.com");
        let first = service.handle(alice).await.unwrap();
        assert_eq!(
            first.choices[0].message.content,
            json!("Response for alice@example.com")
        );

        let mut bob = request();
        bob.messages[0].content = json!("Email bob@example.com");
        let second = service.handle(bob).await.unwrap();
        assert_eq!(
            second.choices[0].message.content,
            json!("Response for bob@example.com")
        );
        assert!(!second.choices[0]
            .message
            .content
            .as_str()
            .unwrap_or_default()
            .contains("alice@example.com"));
        assert_eq!(
            upstream_state.lock().unwrap().call_count,
            1,
            "second request should be exact-cache hit"
        );
    }

    #[tokio::test]
    async fn exact_hit_returns_cached_response_and_skips_upstream() {
        let req = request();
        let cached = response_with_usage("exact-hit", 1000, 500);

        let key = exact_key_for(&req);

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
    async fn guard_placeholder_signature_is_passed_to_semantic_lookup_and_store() {
        let req = request();
        let exact_cache = FakeExactCache::new();
        let semantic_cache = FakeSemanticCache::new();
        let semantic_state = semantic_cache.state();
        let upstream = FakeUpstream::new(response_with_usage("upstream-response", 1000, 500));

        let settings = ChatServiceSettings {
            semantic_cache_enabled: true,
            exact_cache_enabled: true,
            exact_cache_fail_open: true,
            exact_cache_store_enabled: true,
            semantic_cache_store_enabled: true,
            semantic_cache_fail_open: true,
            max_prompt_chars: Some(200_000),
            max_inflight_upstream_requests: 500,
        };

        let service = ChatService::new_with_guards(
            Arc::new(exact_cache),
            Arc::new(semantic_cache),
            Arc::new(upstream),
            Arc::new(SignatureGuard("EMAIL:1|IP:1|PHONE:0|JWT:0|API_KEY:0|BEARER_TOKEN:0|PRIVATE_KEY:0|CREDIT_CARD_LIKE:0|OTHER:0")),
            settings,
            model_prices(),
            Some(EmbeddingPrice {
                usd_per_1m_tokens: 0.020,
            }),
        );

        let result = service.handle(req).await.unwrap();

        assert_eq!(result.id, "upstream-response");
        let semantic = semantic_state.lock().unwrap();
        assert_eq!(semantic.lookup_calls, 1);
        assert_eq!(semantic.store_calls, 1);
        assert_eq!(
            semantic.last_lookup_privacy_placeholder_signature.as_deref(),
            Some("EMAIL:1|IP:1|PHONE:0|JWT:0|API_KEY:0|BEARER_TOKEN:0|PRIVATE_KEY:0|CREDIT_CARD_LIKE:0|OTHER:0")
        );
        assert_eq!(
            semantic.last_store_privacy_placeholder_signature.as_deref(),
            Some("EMAIL:1|IP:1|PHONE:0|JWT:0|API_KEY:0|BEARER_TOKEN:0|PRIVATE_KEY:0|CREDIT_CARD_LIKE:0|OTHER:0")
        );
    }

    #[tokio::test]
    async fn stream_requests_are_rejected_in_core_only_mode() {
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

        let err = service.handle(req).await.unwrap_err();

        assert_eq!(
            err.status_code(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(err.metrics_class(), "validation");
        assert_eq!(
            err.message(),
            "stream=true is not supported by AI Firewall; set stream=false"
        );

        assert_eq!(
            upstream_state.lock().unwrap().call_count,
            0,
            "streaming request should be rejected before upstream"
        );
        assert_eq!(
            exact_state.lock().unwrap().get_calls,
            0,
            "streaming request should be rejected before exact cache lookup"
        );
        assert_eq!(
            semantic_state.lock().unwrap().lookup_calls,
            0,
            "streaming request should be rejected before semantic cache lookup"
        );
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
            max_inflight_upstream_requests: 500,
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
            max_inflight_upstream_requests: 500,
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
            max_inflight_upstream_requests: 500,
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
            max_inflight_upstream_requests: 500,
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
