use std::{collections::HashMap, sync::Arc, time::Instant};

use async_stream::try_stream;
use axum::body::Bytes;
use futures_util::{stream, StreamExt};
use std::io;
use tokio::sync::Semaphore;

use crate::{
    app::DependencyState,
    cache::{exact::ExactCache, redis_exact::RedisOperationTimeout},
    config::{AifEnforcementMode, EmbeddingPrice, ModelPrice},
    core::{
        cache_admission::cacheable_response,
        hashing::sha256_hex,
        normalize::{
            normalize_chat_request, semantic_compatibility_scope, semantic_text_from_request,
        },
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
    streaming::{encode_response_as_sse, stream_include_usage, OpenAiStreamAssembler},
    types::openai::{ChatCompletionRequest, ChatCompletionResponse, ChatMessage, Choice, Usage},
    upstream::llm::{LlmUpstream, UpstreamByteStream, UpstreamErrorKind, UpstreamStreamError},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct CacheControl {
    pub bypass_lookup: bool,
    pub bypass_store: bool,
}

pub struct ChatStreamResponse {
    pub content_type: String,
    pub body: UpstreamByteStream,
}

enum ChatExecution {
    Completed(ChatCompletionResponse),
    Progressive(UpstreamByteStream),
}

struct ProgressiveFinalization {
    request: ChatCompletionRequest,
    exact_key: String,
    semantic_text: Option<String>,
    semantic_compatibility_scope: Option<String>,
    privacy_cache_scope: Option<String>,
    privacy_placeholder_signature: Option<String>,
    store_after_upstream: bool,
    bypass_store: bool,
    evaluation_hit: Option<&'static str>,
    evaluation_semantic_lookup_tokens: u32,
    evaluation_incomplete: bool,
}

impl From<ChatCompletionResponse> for ChatExecution {
    fn from(response: ChatCompletionResponse) -> Self {
        Self::Completed(response)
    }
}

struct StreamTerminalGuard {
    evidence_sink: Arc<dyn EvidenceSink>,
    trace_id: uuid::Uuid,
    started: Instant,
    terminal_emitted: bool,
    enforcement_mode: AifEnforcementMode,
    abort_reason: &'static str,
}

#[derive(Clone, Copy, Debug)]
struct StreamProgress {
    chunks: u64,
    bytes: u64,
}

impl Drop for StreamTerminalGuard {
    fn drop(&mut self) {
        if self.terminal_emitted {
            return;
        }

        metrics::STREAM_ABORTED_TOTAL.inc();
        metrics::STREAM_DURATION_SECONDS.observe(self.started.elapsed().as_secs_f64());

        let evidence_sink = self.evidence_sink.clone();
        let trace_id = self.trace_id;
        let enforcement_mode = self.enforcement_mode;
        let latency_ms = self.started.elapsed().as_millis() as u64;
        let abort_reason = self.abort_reason;

        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };

        runtime.spawn(async move {
            let mut aborted = EvidenceEvent::new(
                trace_id,
                EvidenceSource::AiFirewall,
                EventCategory::Request,
                "request.stream.aborted",
                EventOutcome::Failed,
            );
            aborted.attributes.insert(
                "reason".into(),
                serde_json::Value::String(abort_reason.into()),
            );
            aborted
                .attributes
                .insert("latency_ms".into(), serde_json::Value::from(latency_ms));
            aborted
                .attributes
                .insert("stream".into(), serde_json::Value::Bool(true));
            aborted.attributes.insert(
                "enforcement_mode".into(),
                serde_json::Value::String(enforcement_mode.as_str().into()),
            );
            let _ = evidence_sink.emit(aborted).await;

            let mut terminal = EvidenceEvent::new(
                trace_id,
                EvidenceSource::AiFirewall,
                EventCategory::Request,
                "request.failed",
                EventOutcome::Failed,
            );
            terminal.decision = Some(DecisionEvidence {
                action: "fail_request".into(),
                reason_code: "STREAM_ABORTED".into(),
                rule_id: None,
                severity: None,
            });
            terminal.attributes.insert(
                "error_class".into(),
                serde_json::Value::String("stream_aborted".into()),
            );
            terminal
                .attributes
                .insert("stream".into(), serde_json::Value::Bool(true));
            terminal.attributes.insert(
                "enforcement_mode".into(),
                serde_json::Value::String(enforcement_mode.as_str().into()),
            );
            let _ = evidence_sink.emit(terminal).await;
        });
    }
}

impl StreamTerminalGuard {
    async fn fail_progressive(
        &mut self,
        reason: &'static str,
        upstream_failure: bool,
        partial_content_emitted: bool,
    ) {
        if self.terminal_emitted {
            return;
        }
        self.terminal_emitted = true;
        metrics::STREAM_ERRORS_TOTAL.inc();
        metrics::STREAM_DURATION_SECONDS.observe(self.started.elapsed().as_secs_f64());

        if upstream_failure {
            let mut upstream = EvidenceEvent::new(
                self.trace_id,
                EvidenceSource::AiFirewall,
                EventCategory::Upstream,
                "upstream.stream.failed",
                EventOutcome::Failed,
            );
            upstream
                .attributes
                .insert("delivery_mode".into(), serde_json::json!("progressive"));
            upstream
                .attributes
                .insert("error_class".into(), serde_json::json!(reason));
            upstream.attributes.insert(
                "partial_content_emitted".into(),
                serde_json::json!(partial_content_emitted),
            );
            let _ = self.evidence_sink.emit(upstream).await;
        }

        let mut failed = EvidenceEvent::new(
            self.trace_id,
            EvidenceSource::AiFirewall,
            EventCategory::Request,
            "request.failed",
            EventOutcome::Failed,
        );
        failed.decision = Some(DecisionEvidence {
            action: "fail_request".into(),
            reason_code: "STREAM_FAILED".into(),
            rule_id: None,
            severity: None,
        });
        failed
            .attributes
            .insert("error_class".into(), serde_json::json!(reason));
        failed
            .attributes
            .insert("delivery_mode".into(), serde_json::json!("progressive"));
        failed
            .attributes
            .insert("stream".into(), serde_json::json!(true));
        failed.attributes.insert(
            "partial_content_emitted".into(),
            serde_json::json!(partial_content_emitted),
        );
        failed.attributes.insert(
            "enforcement_mode".into(),
            serde_json::json!(self.enforcement_mode.as_str()),
        );
        let _ = self.evidence_sink.emit(failed).await;
    }
}

#[derive(Clone)]
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
    enforcement_mode: AifEnforcementMode,
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
    pub enforcement_mode: AifEnforcementMode,
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

fn event_is_usage_only(event: &Bytes) -> bool {
    let Ok(text) = std::str::from_utf8(event) else {
        return false;
    };
    let Some(payload) = text.strip_prefix("data: ") else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload.trim()) else {
        return false;
    };
    value.get("usage").is_some_and(|usage| !usage.is_null())
        && value
            .get("choices")
            .and_then(|choices| choices.as_array())
            .is_some_and(Vec::is_empty)
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

fn request_policy_completion(
    req: &ChatCompletionRequest,
    trace_id: uuid::Uuid,
    id_scope: &str,
    message: &str,
) -> ChatCompletionResponse {
    let created = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX);

    ChatCompletionResponse {
        id: format!("chatcmpl-vcal-{}-{}", id_scope, trace_id.simple()),
        object: "chat.completion".to_string(),
        created,
        model: req.model.clone(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: serde_json::Value::String(message.to_string()),
                name: None,
                extra: serde_json::Map::new(),
            },
            finish_reason: Some("stop".to_string()),
            extra: serde_json::Map::new(),
        }],
        usage: Some(Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            extra: serde_json::Map::new(),
        }),
        extra: serde_json::Map::new(),
    }
}

fn is_security_policy_completion(response: &ChatCompletionResponse) -> bool {
    response.id.starts_with("chatcmpl-vcal-security-")
}

fn completed_delivery_path(
    response: &ChatCompletionResponse,
    normal_path: &'static str,
) -> &'static str {
    if is_security_policy_completion(response) {
        "security_policy"
    } else {
        normal_path
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
                enforcement_mode: AifEnforcementMode::Enforce,
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
        let track_redis_runtime = settings.exact_cache_enabled;
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
            enforcement_mode: deps.enforcement_mode,
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

    pub async fn list_models(&self) -> Result<serde_json::Value, AppError> {
        self.upstream.list_models().await
    }

    #[cfg(test)]
    pub async fn handle(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, AppError> {
        self.handle_with_evidence(req, CacheControl::default(), uuid::Uuid::new_v4())
            .await
    }

    pub async fn handle_stream_with_evidence(
        &self,
        req: ChatCompletionRequest,
        cache_control: CacheControl,
        trace_id: uuid::Uuid,
        max_stream_upstream_bytes: usize,
        progressive_requested: bool,
    ) -> Result<ChatStreamResponse, AppError> {
        self.validate(&req)?;

        if !req.stream.unwrap_or(false) {
            return Err(AppError::internal(
                "controlled streaming execution requires stream=true",
            ));
        }

        metrics::STREAM_REQUESTS_TOTAL.inc();
        let started = Instant::now();

        let mut received = EvidenceEvent::new(
            trace_id,
            EvidenceSource::AiFirewall,
            EventCategory::Request,
            "request.received",
            EventOutcome::Started,
        );
        received
            .attributes
            .insert("stream".into(), serde_json::Value::Bool(true));
        received.attributes.insert(
            "delivery_mode".into(),
            serde_json::Value::String(
                if progressive_requested {
                    "requested_progressive"
                } else {
                    "controlled"
                }
                .into(),
            ),
        );
        self.emit(received).await;

        let mut lifecycle = StreamTerminalGuard {
            evidence_sink: self.evidence_sink.clone(),
            trace_id,
            started,
            terminal_emitted: false,
            enforcement_mode: self.enforcement_mode,
            abort_reason: "client_disconnected_before_commit",
        };

        let include_usage = stream_include_usage(&req);
        let result = self
            .handle_after_request_received(
                req,
                cache_control,
                trace_id,
                Some(max_stream_upstream_bytes),
                progressive_requested,
            )
            .await;

        let response = match result {
            Ok(ChatExecution::Completed(response)) => response,
            Ok(ChatExecution::Progressive(body)) => {
                lifecycle.terminal_emitted = true;
                return Ok(ChatStreamResponse {
                    content_type: "text/event-stream".into(),
                    body,
                });
            }
            Err(error) => {
                metrics::STREAM_ERRORS_TOTAL.inc();
                metrics::STREAM_DURATION_SECONDS.observe(started.elapsed().as_secs_f64());

                let mut failed = self.request_failed_event(trace_id, &error);
                failed
                    .attributes
                    .insert("stream".into(), serde_json::Value::Bool(true));
                self.emit(failed).await;

                lifecycle.terminal_emitted = true;
                return Err(error);
            }
        };

        let body = encode_response_as_sse(&response, include_usage);
        metrics::STREAM_BUFFER_BYTES.observe(body.len() as f64);
        metrics::STREAM_COMPLETED_TOTAL.inc();
        metrics::STREAM_DURATION_SECONDS.observe(started.elapsed().as_secs_f64());

        lifecycle.terminal_emitted = true;

        let client_started = started;
        let body = stream::once(async move {
            metrics::STREAM_CLIENT_TIME_TO_FIRST_BYTE_SECONDS
                .observe(client_started.elapsed().as_secs_f64());
            Ok::<Bytes, UpstreamStreamError>(body)
        });

        Ok(ChatStreamResponse {
            content_type: "text/event-stream".into(),
            body: Box::pin(body),
        })
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
            .handle_after_request_received(req, cache_control, trace_id, None, false)
            .await
            .and_then(|outcome| match outcome {
                ChatExecution::Completed(response) => Ok(response),
                ChatExecution::Progressive(_) => Err(AppError::internal(
                    "non-streaming request returned a progressive response",
                )),
            });

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
        controlled_stream_max_bytes: Option<usize>,
        progressive_requested: bool,
    ) -> Result<ChatExecution, AppError> {
        let controlled_stream = controlled_stream_max_bytes.is_some();
        let observe_mode = self.enforcement_mode.is_observe();

        if req.stream.unwrap_or(false) != controlled_stream {
            return Err(AppError::internal(
                "chat request reached the wrong streaming execution path",
            ));
        }

        let guarded = self.guard_orchestrator.before_cache(req, trace_id).await?;

        if let Some(message) = guarded.context.security_policy_block_message.as_deref() {
            tracing::debug!(
                model = %guarded.request.normalized_model(),
                "Security Guard request block returned as synthetic completion"
            );
            let response =
                request_policy_completion(&guarded.request, trace_id, "security", message);
            self.emit(self.request_completed_event(trace_id, "security_policy", controlled_stream))
                .await;
            return Ok(response.into());
        }

        if let Some(message) = guarded.context.usage_policy_block_message.as_deref() {
            tracing::debug!(
                model = %guarded.request.normalized_model(),
                "Usage Guard policy stop returned as synthetic completion"
            );
            let response = request_policy_completion(&guarded.request, trace_id, "policy", message);
            self.emit(self.request_completed_event(trace_id, "usage_policy", controlled_stream))
                .await;
            return Ok(response.into());
        }

        let req = guarded.request;
        let guard_context = guarded.context;
        let cache_control = CacheControl {
            bypass_lookup: cache_control.bypass_lookup || guarded.cache_control.bypass_lookup,
            bypass_store: cache_control.bypass_store || guarded.cache_control.bypass_store,
        };

        if observe_mode {
            metrics::EVALUATION_REQUESTS_TOTAL.inc();
        }

        let mut cache_req = req.clone();
        cache_req.stream = None;
        cache_req.extra.remove("stream_options");

        let normalized = normalize_chat_request(&cache_req)
            .map_err(|e| AppError::bad_request(format!("normalize failed: {e}")))?;
        let semantic_eligible = self.semantic_eligible(&cache_req);
        let semantic_text = semantic_eligible.then(|| semantic_text_from_request(&cache_req));
        let semantic_scope = semantic_eligible
            .then(|| semantic_compatibility_scope(&cache_req))
            .transpose()
            .map_err(|e| {
                AppError::bad_request(format!("semantic compatibility scope failed: {e}"))
            })?;
        let privacy_cache_scope = guard_context.privacy_cache_scope.as_deref();
        let privacy_placeholder_signature = guard_context.privacy_placeholder_signature.as_deref();

        let exact_identity = match privacy_cache_scope {
            Some(scope) => format!("{normalized}\nprivacy_cache_scope={scope}"),
            None => normalized.clone(),
        };
        let exact_key_hash = sha256_hex(&exact_identity);
        let exact_key = format!("{}:{exact_key_hash}", self.effective_exact_cache_prefix());
        let mut evaluation_hit: Option<&'static str> = None;
        let mut evaluation_semantic_lookup_tokens = 0u32;
        let mut evaluation_incomplete = false;

        if self.exact_cache_enabled && !cache_control.bypass_lookup {
            if observe_mode && !self.track_redis_runtime {
                evaluation_incomplete = true;
                self.record_evaluation_error("redis", "lookup");
                let mut event = EvidenceEvent::new(
                    trace_id,
                    EvidenceSource::AiFirewall,
                    EventCategory::Cache,
                    "cache.exact.lookup",
                    EventOutcome::Failed,
                );
                event.cache = Some(CacheEvidence {
                    cache_type: "exact".into(),
                    operation: "lookup".into(),
                    outcome: EventOutcome::Failed,
                    cache_key_hash: Some(exact_key_hash.clone()),
                    record_id: None,
                    similarity_score: None,
                    threshold: None,
                    upstream_called: true,
                });
                self.annotate_cache_decision(
                    &mut event,
                    "evaluation_unavailable",
                    "unknown",
                    "call_upstream",
                );
                event.attributes.insert(
                    "evaluation_error".into(),
                    serde_json::Value::String("redis_unavailable_at_startup".into()),
                );
                self.emit(event).await;
            } else {
                let exact_lookup = self.exact_cache.get(&exact_key).await.map(|candidate| {
                    candidate.and_then(|raw| {
                        match serde_json::from_str::<ChatCompletionResponse>(&raw) {
                            Ok(response) if cacheable_response(&response) => Some(raw),
                            _ => {
                                tracing::warn!(
                                    "rejecting invalid or non-reusable existing exact cache entry"
                                );
                                None
                            }
                        }
                    })
                });
                match exact_lookup {
                    Ok(Some(raw)) => {
                        let hit: ChatCompletionResponse =
                            serde_json::from_str(&raw).map_err(|e| {
                                AppError::internal(format!("cached response decode failed: {e}"))
                            })?;

                        tracing::debug!(
                            model = %req.normalized_model(),
                            cache_key = %exact_key,
                            enforcement_mode = self.enforcement_mode.as_str(),
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
                            upstream_called: observe_mode,
                        });

                        if observe_mode {
                            self.record_evaluation_cache_outcome(CACHE_TYPE_EXACT, "hit");
                            self.record_evaluation_upstream_call_avoided(CACHE_TYPE_EXACT);
                            self.annotate_cache_decision(
                                &mut event,
                                "exact_hit",
                                "serve_exact_cache",
                                "call_upstream",
                            );
                            self.emit(event).await;
                            evaluation_hit = Some(CACHE_TYPE_EXACT);
                        } else {
                            metrics::CACHE_EXACT_HITS.inc();
                            self.record_exact_hit_savings(&hit);
                            self.annotate_cache_decision(
                                &mut event,
                                "exact_hit",
                                "serve_exact_cache",
                                "serve_exact_cache",
                            );
                            self.emit(event).await;

                            let response = self
                                .security_scan_response(&guard_context, hit, trace_id)
                                .await?;

                            if is_security_policy_completion(&response) {
                                self.emit(self.request_completed_event(
                                    trace_id,
                                    "security_policy",
                                    controlled_stream,
                                ))
                                .await;
                                return Ok(response.into());
                            }

                            let response = self
                                .restore_guarded_response(&guard_context, response, trace_id)
                                .await?;

                            self.emit(self.request_completed_event(
                                trace_id,
                                "exact_cache",
                                controlled_stream,
                            ))
                            .await;

                            return Ok(response.into());
                        }
                    }
                    Ok(None) => {
                        if observe_mode {
                            self.record_evaluation_cache_outcome(CACHE_TYPE_EXACT, "miss");
                        }
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
                        self.annotate_cache_decision(
                            &mut event,
                            "exact_miss",
                            "continue_cache_pipeline",
                            "continue_cache_pipeline",
                        );
                        self.emit(event).await;
                    }
                    Err(e) if self.exact_cache_fail_open || observe_mode => {
                        self.set_redis_available(false);
                        if observe_mode {
                            evaluation_incomplete = true;
                            self.record_evaluation_error("redis", "lookup");
                        }
                        tracing::warn!(
                            model = %req.normalized_model(),
                            error = %e,
                            enforcement_mode = self.enforcement_mode.as_str(),
                            "exact cache lookup failed; request continues"
                        );
                        if observe_mode {
                            let mut event = EvidenceEvent::new(
                                trace_id,
                                EvidenceSource::AiFirewall,
                                EventCategory::Cache,
                                "cache.exact.lookup",
                                EventOutcome::Failed,
                            );
                            event.cache = Some(CacheEvidence {
                                cache_type: "exact".into(),
                                operation: "lookup".into(),
                                outcome: EventOutcome::Failed,
                                cache_key_hash: Some(exact_key_hash.clone()),
                                record_id: None,
                                similarity_score: None,
                                threshold: None,
                                upstream_called: true,
                            });
                            self.annotate_cache_decision(
                                &mut event,
                                "evaluation_error",
                                "unknown",
                                "call_upstream",
                            );
                            event.attributes.insert(
                                "evaluation_error".into(),
                                serde_json::Value::String(e.to_string()),
                            );
                            self.emit(event).await;
                        }
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
            }
        } else if cache_control.bypass_lookup {
            tracing::debug!(
                model = %req.normalized_model(),
                "cache lookup bypass requested"
            );
            let mut event = EvidenceEvent::new(
                trace_id,
                EvidenceSource::AiFirewall,
                EventCategory::Cache,
                "cache.lookup.bypassed",
                EventOutcome::Bypassed,
            );
            self.annotate_cache_decision(
                &mut event,
                "cache_bypassed",
                "call_upstream",
                "call_upstream",
            );
            self.emit(event).await;
        }

        if self.semantic_cache_enabled
            && !cache_control.bypass_lookup
            && evaluation_hit.is_none()
            && semantic_eligible
        {
            if observe_mode && !self.track_qdrant_runtime {
                evaluation_incomplete = true;
                self.record_evaluation_error("qdrant", "lookup");
                let mut event = EvidenceEvent::new(
                    trace_id,
                    EvidenceSource::AiFirewall,
                    EventCategory::Cache,
                    "cache.semantic.lookup",
                    EventOutcome::Failed,
                );
                event.cache = Some(CacheEvidence {
                    cache_type: "semantic".into(),
                    operation: "lookup".into(),
                    outcome: EventOutcome::Failed,
                    cache_key_hash: None,
                    record_id: None,
                    similarity_score: None,
                    threshold: None,
                    upstream_called: true,
                });
                self.annotate_cache_decision(
                    &mut event,
                    "evaluation_unavailable",
                    "unknown",
                    "call_upstream",
                );
                event.attributes.insert(
                    "evaluation_error".into(),
                    serde_json::Value::String("qdrant_unavailable_at_startup".into()),
                );
                self.emit(event).await;
            } else {
                match self
                    .semantic_cache
                    .lookup(
                        req.normalized_model(),
                        semantic_text
                            .as_deref()
                            .expect("semantic text must exist for eligible requests"),
                        semantic_scope
                            .as_deref()
                            .expect("scope for eligible request"),
                        privacy_cache_scope,
                        privacy_placeholder_signature,
                    )
                    .await
                    .map(|candidate| candidate.filter(|hit| cacheable_response(&hit.response)))
                {
                    Ok(Some(hit)) => {
                        self.set_qdrant_available(true);

                        tracing::debug!(
                            model = %req.normalized_model(),
                            enforcement_mode = self.enforcement_mode.as_str(),
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
                            upstream_called: observe_mode,
                        });

                        let embedding_prompt_tokens = hit
                            .embedding_usage
                            .as_ref()
                            .map(|usage| usage.prompt_tokens)
                            .unwrap_or(0);

                        if observe_mode {
                            self.record_evaluation_cache_outcome(CACHE_TYPE_SEMANTIC, "hit");
                            self.record_evaluation_upstream_call_avoided(CACHE_TYPE_SEMANTIC);
                            evaluation_semantic_lookup_tokens = embedding_prompt_tokens;
                            if embedding_prompt_tokens > 0 {
                                self.record_evaluation_semantic_lookup_overhead(
                                    req.normalized_model(),
                                    embedding_prompt_tokens,
                                );
                                self.record_embedding_overhead(
                                    req.normalized_model(),
                                    EMBEDDING_OPERATION_LOOKUP,
                                    embedding_prompt_tokens,
                                );
                            }
                            self.annotate_cache_decision(
                                &mut event,
                                "semantic_hit",
                                "serve_semantic_cache",
                                "call_upstream",
                            );
                            self.emit(event).await;

                            if self.exact_cache_enabled
                                && self.exact_cache_store_enabled
                                && !cache_control.bypass_store
                            {
                                if !self.track_redis_runtime {
                                    evaluation_incomplete = true;
                                    self.record_evaluation_error("redis", "warm_exact");
                                    self.record_evaluation_shadow_store(CACHE_TYPE_EXACT, "error");
                                } else if let Ok(raw) = serde_json::to_string(&hit.response) {
                                    match self.exact_cache.set(&exact_key, raw).await {
                                        Ok(()) => {
                                            self.set_redis_available(true);
                                            self.record_evaluation_shadow_store(
                                                CACHE_TYPE_EXACT,
                                                "stored",
                                            );
                                        }
                                        Err(e) => {
                                            self.set_redis_available(false);
                                            evaluation_incomplete = true;
                                            self.record_evaluation_error("redis", "warm_exact");
                                            self.record_evaluation_shadow_store(
                                                CACHE_TYPE_EXACT,
                                                "error",
                                            );
                                            tracing::warn!(
                                                error = %e,
                                                "failed to warm shadow exact cache from semantic hit"
                                            );
                                        }
                                    }
                                } else {
                                    evaluation_incomplete = true;
                                    self.record_evaluation_error("exact_cache", "serialize_warm");
                                    self.record_evaluation_shadow_store(CACHE_TYPE_EXACT, "error");
                                }
                            }

                            evaluation_hit = Some(CACHE_TYPE_SEMANTIC);
                        } else {
                            metrics::CACHE_SEMANTIC_HITS.inc();
                            self.record_semantic_hit_savings(
                                &hit.response,
                                embedding_prompt_tokens,
                            );
                            self.annotate_cache_decision(
                                &mut event,
                                "semantic_hit",
                                "serve_semantic_cache",
                                "serve_semantic_cache",
                            );
                            self.emit(event).await;

                            let response = self
                                .security_scan_response(&guard_context, hit.response, trace_id)
                                .await?;

                            if is_security_policy_completion(&response) {
                                self.emit(self.request_completed_event(
                                    trace_id,
                                    "security_policy",
                                    controlled_stream,
                                ))
                                .await;
                                return Ok(response.into());
                            }

                            if self.exact_cache_enabled
                                && self.exact_cache_store_enabled
                                && !cache_control.bypass_store
                            {
                                if let Ok(raw) = serde_json::to_string(&response) {
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
                                                format!(
                                                    "exact cache set failed while warming semantic hit: {e}"
                                                ),
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
                                .restore_guarded_response(&guard_context, response, trace_id)
                                .await?;

                            self.emit(self.request_completed_event(
                                trace_id,
                                "semantic_cache",
                                controlled_stream,
                            ))
                            .await;

                            return Ok(response.into());
                        }
                    }

                    Ok(None) => {
                        self.set_qdrant_available(true);
                        if observe_mode {
                            self.record_evaluation_cache_outcome(CACHE_TYPE_SEMANTIC, "miss");
                        }
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
                        self.annotate_cache_decision(
                            &mut event,
                            "semantic_miss",
                            "call_upstream",
                            "call_upstream",
                        );
                        self.emit(event).await;
                    }

                    Err(e) if self.semantic_cache_fail_open || observe_mode => {
                        self.set_qdrant_available(false);
                        self.record_semantic_skip("lookup_error");
                        if observe_mode {
                            evaluation_incomplete = true;
                            self.record_evaluation_error("qdrant", "lookup");
                        }

                        tracing::warn!(
                            model = %req.normalized_model(),
                            error = %e,
                            enforcement_mode = self.enforcement_mode.as_str(),
                            "semantic lookup failed; request continues upstream"
                        );

                        if observe_mode {
                            let mut event = EvidenceEvent::new(
                                trace_id,
                                EvidenceSource::AiFirewall,
                                EventCategory::Cache,
                                "cache.semantic.lookup",
                                EventOutcome::Failed,
                            );
                            event.cache = Some(CacheEvidence {
                                cache_type: "semantic".into(),
                                operation: "lookup".into(),
                                outcome: EventOutcome::Failed,
                                cache_key_hash: None,
                                record_id: None,
                                similarity_score: None,
                                threshold: None,
                                upstream_called: true,
                            });
                            self.annotate_cache_decision(
                                &mut event,
                                "evaluation_error",
                                "unknown",
                                "call_upstream",
                            );
                            event.attributes.insert(
                                "evaluation_error".into(),
                                serde_json::Value::String(e.to_string()),
                            );
                            self.emit(event).await;
                        }
                    }

                    Err(e) => {
                        self.set_qdrant_available(false);
                        return Err(AppError::dependency_failure(
                            DependencyKind::Qdrant,
                            FailureClass::Unavailable,
                            format!(
                                "semantic lookup failed and semantic_cache_fail_open=false: {e}"
                            ),
                        ));
                    }
                }
            }
        } else if self.semantic_cache_enabled
            && !cache_control.bypass_lookup
            && evaluation_hit.is_none()
            && !semantic_eligible
        {
            self.record_semantic_skip("ineligible_request");

            tracing::debug!(
                model = %req.normalized_model(),
                "semantic cache skipped because request is ineligible"
            );
        }

        if observe_mode {
            let (outcome, decision, would_action) = match evaluation_hit {
                Some(CACHE_TYPE_EXACT) => (EventOutcome::Hit, "exact_hit", "serve_exact_cache"),
                Some(CACHE_TYPE_SEMANTIC) => {
                    (EventOutcome::Hit, "semantic_hit", "serve_semantic_cache")
                }
                _ if evaluation_incomplete => {
                    (EventOutcome::Failed, "evaluation_incomplete", "unknown")
                }
                _ => (EventOutcome::Miss, "cache_miss", "call_upstream"),
            };
            let mut completed = EvidenceEvent::new(
                trace_id,
                EvidenceSource::AiFirewall,
                EventCategory::Cache,
                "cache.lookup.completed",
                outcome,
            );
            self.annotate_cache_decision(&mut completed, decision, would_action, "call_upstream");
            completed.attributes.insert(
                "evaluation_incomplete".into(),
                serde_json::Value::Bool(evaluation_incomplete),
            );
            self.emit(completed).await;
        } else {
            metrics::CACHE_MISSES.inc();
            let mut completed = EvidenceEvent::new(
                trace_id,
                EvidenceSource::AiFirewall,
                EventCategory::Cache,
                "cache.lookup.completed",
                EventOutcome::Miss,
            );
            self.annotate_cache_decision(
                &mut completed,
                "cache_miss",
                "call_upstream",
                "call_upstream",
            );
            self.emit(completed).await;
        }

        tracing::debug!(
            model = %req.normalized_model(),
            controlled_stream = controlled_stream,
            enforcement_mode = self.enforcement_mode.as_str(),
            evaluation_hit = evaluation_hit.unwrap_or("none"),
            "forwarding request upstream"
        );

        let progressive_store_safe = observe_mode
            || ((!self.exact_cache_enabled
                || !self.exact_cache_store_enabled
                || self.exact_cache_fail_open
                || cache_control.bypass_store)
                && (!self.semantic_cache_enabled
                    || !self.semantic_cache_store_enabled
                    || self.semantic_cache_fail_open
                    || cache_control.bypass_store));
        if progressive_requested
            && progressive_store_safe
            && self.guard_orchestrator.supports_progressive_response()
        {
            let max_bytes = controlled_stream_max_bytes
                .ok_or_else(|| AppError::internal("progressive execution requires stream=true"))?;
            let finalization = ProgressiveFinalization {
                request: req.clone(),
                exact_key: exact_key.clone(),
                semantic_text: semantic_text.clone(),
                semantic_compatibility_scope: semantic_scope.clone(),
                privacy_cache_scope: privacy_cache_scope.map(str::to_owned),
                privacy_placeholder_signature: privacy_placeholder_signature.map(str::to_owned),
                store_after_upstream: !observe_mode || evaluation_hit.is_none(),
                bypass_store: cache_control.bypass_store,
                evaluation_hit,
                evaluation_semantic_lookup_tokens,
                evaluation_incomplete,
            };
            return self
                .prepare_progressive_upstream(&req, trace_id, max_bytes, finalization)
                .await
                .map(ChatExecution::Progressive);
        }

        let response = if let Some(max_stream_upstream_bytes) = controlled_stream_max_bytes {
            self.fetch_controlled_stream_response(&req, trace_id, max_stream_upstream_bytes)
                .await?
        } else {
            self.fetch_buffered_upstream_response(&req, trace_id)
                .await?
        };

        self.record_upstream_model_cost(&response);

        if observe_mode {
            match evaluation_hit {
                Some(CACHE_TYPE_EXACT) => self.record_evaluation_exact_hit_savings(&response),
                Some(CACHE_TYPE_SEMANTIC) => self.record_evaluation_semantic_hit_savings(
                    &response,
                    evaluation_semantic_lookup_tokens,
                ),
                _ => {}
            }
        }

        let response = self
            .security_scan_response(&guard_context, response, trace_id)
            .await?;

        if is_security_policy_completion(&response) {
            self.emit(self.request_completed_event(trace_id, "security_policy", controlled_stream))
                .await;
            return Ok(response.into());
        }

        let store_after_upstream = !observe_mode || evaluation_hit.is_none();

        if store_after_upstream
            && cacheable_response(&response)
            && self.exact_cache_enabled
            && self.exact_cache_store_enabled
            && !cache_control.bypass_store
        {
            if observe_mode && !self.track_redis_runtime {
                evaluation_incomplete = true;
                self.record_evaluation_error("redis", "store");
                self.record_evaluation_shadow_store(CACHE_TYPE_EXACT, "error");
            } else {
                let raw = serde_json::to_string(&response)
                    .map_err(|e| AppError::internal(format!("response encode failed: {e}")))?;
                match self.exact_cache.set(&exact_key, raw).await {
                    Ok(()) => {
                        self.set_redis_available(true);
                        if observe_mode {
                            self.record_evaluation_shadow_store(CACHE_TYPE_EXACT, "stored");
                        }
                    }
                    Err(e) if self.exact_cache_fail_open || observe_mode => {
                        self.set_redis_available(false);
                        if observe_mode {
                            evaluation_incomplete = true;
                            self.record_evaluation_error("redis", "store");
                            self.record_evaluation_shadow_store(CACHE_TYPE_EXACT, "error");
                        }
                        tracing::warn!(
                            model = %req.normalized_model(),
                            error = %e,
                            enforcement_mode = self.enforcement_mode.as_str(),
                            "exact cache store failed; response is returned"
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
        }

        if store_after_upstream
            && cacheable_response(&response)
            && self.semantic_cache_enabled
            && self.semantic_cache_store_enabled
            && !cache_control.bypass_store
            && semantic_eligible
        {
            if observe_mode && !self.track_qdrant_runtime {
                evaluation_incomplete = true;
                self.record_evaluation_error("qdrant", "store");
                self.record_evaluation_shadow_store(CACHE_TYPE_SEMANTIC, "error");
            } else {
                match self
                    .semantic_cache
                    .store(
                        req.normalized_model(),
                        semantic_text
                            .as_deref()
                            .expect("semantic text must exist for eligible requests"),
                        &response,
                        semantic_scope
                            .as_deref()
                            .expect("scope for eligible request"),
                        privacy_cache_scope,
                        privacy_placeholder_signature,
                    )
                    .await
                {
                    Ok(embedding_usage) => {
                        self.set_qdrant_available(true);
                        if observe_mode {
                            self.record_evaluation_shadow_store(CACHE_TYPE_SEMANTIC, "stored");
                        }
                        if let Some(usage) = embedding_usage {
                            self.record_embedding_overhead(
                                req.normalized_model(),
                                EMBEDDING_OPERATION_STORE,
                                usage.prompt_tokens,
                            );
                        }
                    }

                    Err(e) if self.semantic_cache_fail_open || observe_mode => {
                        self.set_qdrant_available(false);
                        self.record_semantic_skip("store_error");
                        if observe_mode {
                            evaluation_incomplete = true;
                            self.record_evaluation_error("qdrant", "store");
                            self.record_evaluation_shadow_store(CACHE_TYPE_SEMANTIC, "error");
                        }

                        tracing::warn!(
                            model = %req.normalized_model(),
                            error = %e,
                            enforcement_mode = self.enforcement_mode.as_str(),
                            "semantic store failed; response returned without semantic cache write"
                        );
                    }

                    Err(e) => {
                        self.set_qdrant_available(false);
                        self.record_semantic_skip("store_error");

                        return Err(AppError::dependency_failure(
                            DependencyKind::Qdrant,
                            FailureClass::Unavailable,
                            format!(
                                "semantic store failed and semantic_cache_fail_open=false: {e}"
                            ),
                        ));
                    }
                }
            }
        }

        if observe_mode && evaluation_incomplete {
            tracing::debug!(
                trace_id = %trace_id,
                "evaluation completed with one or more non-blocking cache dependency errors"
            );
        }

        let response = self
            .restore_guarded_response(&guard_context, response, trace_id)
            .await?;
        let delivery_path = completed_delivery_path(&response, "upstream");
        self.emit(self.request_completed_event(trace_id, delivery_path, controlled_stream))
            .await;
        Ok(response.into())
    }

    async fn fetch_buffered_upstream_response(
        &self,
        req: &ChatCompletionRequest,
        trace_id: uuid::Uuid,
    ) -> Result<ChatCompletionResponse, AppError> {
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
        let response = match self.upstream.chat_completion(req).await {
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
                    latency_ms: Some(upstream_started.elapsed().as_millis() as u64),
                });
                upstream_failed.decision = Some(DecisionEvidence {
                    action: "fail_request".into(),
                    reason_code: error.evidence_reason_code().into(),
                    rule_id: None,
                    severity: None,
                });
                self.emit(upstream_failed).await;
                return Err(error);
            }
        };

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
            latency_ms: Some(upstream_started.elapsed().as_millis() as u64),
        });
        self.emit(upstream_received).await;

        Ok(response)
    }

    async fn prepare_progressive_upstream(
        &self,
        req: &ChatCompletionRequest,
        trace_id: uuid::Uuid,
        max_bytes: usize,
        finalization: ProgressiveFinalization,
    ) -> Result<UpstreamByteStream, AppError> {
        let _permit = self
            .upstream_limit
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                metrics::observe_backpressure_rejection("upstream");
                AppError::backpressure("upstream", "upstream concurrency limit reached")
            })?;

        let mut upstream_req = req.clone();
        upstream_req.stream = Some(true);
        let options = upstream_req
            .extra
            .entry("stream_options".to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if !options.is_object() {
            *options = serde_json::Value::Object(serde_json::Map::new());
        }
        if let Some(options) = options.as_object_mut() {
            options.insert("include_usage".into(), serde_json::Value::Bool(true));
        }

        let mut sent = EvidenceEvent::new(
            trace_id,
            EvidenceSource::AiFirewall,
            EventCategory::Upstream,
            "upstream.request.sent",
            EventOutcome::Started,
        );
        sent.attributes.insert(
            "delivery_mode".into(),
            serde_json::Value::String("progressive".into()),
        );
        self.emit(sent).await;

        let upstream_started = Instant::now();
        let upstream = match self.upstream.chat_completion_stream(&upstream_req).await {
            Ok(upstream) => upstream,
            Err(error) => {
                self.dependencies
                    .upstream_available
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                metrics::STREAM_UPSTREAM_ERRORS_TOTAL.inc();
                let mut failed = EvidenceEvent::new(
                    trace_id,
                    EvidenceSource::AiFirewall,
                    EventCategory::Upstream,
                    "upstream.request.failed",
                    EventOutcome::Failed,
                );
                failed
                    .attributes
                    .insert("delivery_mode".into(), serde_json::json!("progressive"));
                failed.attributes.insert(
                    "error_class".into(),
                    serde_json::json!(error.metrics_class()),
                );
                self.emit(failed).await;
                return Err(error);
            }
        };

        let mut received = EvidenceEvent::new(
            trace_id,
            EvidenceSource::AiFirewall,
            EventCategory::Upstream,
            "upstream.response.received",
            EventOutcome::Started,
        );
        received
            .attributes
            .insert("phase".into(), serde_json::Value::String("headers".into()));
        received.attributes.insert(
            "delivery_mode".into(),
            serde_json::Value::String("progressive".into()),
        );
        self.emit(received).await;

        let mut body = upstream.body;
        let idle_timeout = self.upstream.stream_idle_timeout();
        let generation_timeout = self.upstream.max_stream_generation_duration();
        let model = req.normalized_model().to_string();
        let include_usage = stream_include_usage(req);
        let dependencies = self.dependencies.clone();
        let evidence_sink = self.evidence_sink.clone();
        let enforcement_mode = self.enforcement_mode;
        let finalizer = self.clone();
        let client_started = Instant::now();
        let delivery_started = Instant::now();
        let stream = try_stream! {
            let _permit = _permit;
            let mut terminal = StreamTerminalGuard {
                evidence_sink: evidence_sink.clone(),
                trace_id,
                started: delivery_started,
                terminal_emitted: false,
                enforcement_mode,
                abort_reason: "client_disconnected_during_progressive_stream",
            };
            let mut assembler = OpenAiStreamAssembler::new(model, max_bytes);
            let deadline = tokio::time::Instant::now() + generation_timeout;
            let mut first_upstream = false;
            let mut first_client = false;
            loop {
                let idle_deadline = tokio::time::Instant::now() + idle_timeout;
                let next_deadline = std::cmp::min(deadline, idle_deadline);
                let next = match tokio::time::timeout_at(next_deadline, body.next()).await {
                    Ok(value) => value,
                    Err(_) => {
                        metrics::UPSTREAM_TIMEOUTS_TOTAL.inc();
                        metrics::STREAM_UPSTREAM_ERRORS_TOTAL.inc();
                        dependencies.upstream_available.store(false, std::sync::atomic::Ordering::Relaxed);
                        let reason = if deadline <= idle_deadline { "stream_generation_timeout" } else { "stream_idle_timeout" };
                        terminal.fail_progressive(reason, true, first_client).await;
                        Err(io::Error::new(io::ErrorKind::TimedOut, reason))?
                    }
                };
                let Some(next) = next else { break; };
                let bytes = match next {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        metrics::STREAM_UPSTREAM_ERRORS_TOTAL.inc();
                        dependencies.upstream_available.store(false, std::sync::atomic::Ordering::Relaxed);
                        terminal.fail_progressive("upstream_stream_error", true, first_client).await;
                        Err(io::Error::other(format!("upstream SSE body failed: {error}")))?
                    }
                };
                if !first_upstream {
                    metrics::STREAM_UPSTREAM_TIME_TO_FIRST_BYTE_SECONDS.observe(upstream_started.elapsed().as_secs_f64());
                    first_upstream = true;
                }
                metrics::STREAM_UPSTREAM_CHUNKS_TOTAL.inc();
                metrics::STREAM_UPSTREAM_BYTES_TOTAL.inc_by(bytes.len() as u64);
                let events = match assembler.push_bytes_with_events(&bytes) {
                    Ok(events) => events,
                    Err(error) => {
                        dependencies.upstream_available.store(false, std::sync::atomic::Ordering::Relaxed);
                        metrics::STREAM_UPSTREAM_ERRORS_TOTAL.inc();
                        terminal.fail_progressive("upstream_invalid_sse", true, first_client).await;
                        Err(io::Error::new(io::ErrorKind::InvalidData, error))?
                    }
                };
                for event in events {
                    if !include_usage && event_is_usage_only(&event) {
                        continue;
                    }
                    if !first_client {
                        metrics::STREAM_CLIENT_TIME_TO_FIRST_BYTE_SECONDS.observe(client_started.elapsed().as_secs_f64());
                        first_client = true;
                    }
                    yield event;
                }
            }
            let upstream_bytes = assembler.received_bytes();
            let response = match assembler.finish() {
                Ok(response) => response,
                Err(error) => {
                    metrics::STREAM_UPSTREAM_ERRORS_TOTAL.inc();
                    dependencies.upstream_available.store(false, std::sync::atomic::Ordering::Relaxed);
                    terminal.fail_progressive("upstream_incomplete_sse", true, first_client).await;
                    Err(io::Error::new(io::ErrorKind::InvalidData, error))?
                }
            };
            dependencies.upstream_available.store(true, std::sync::atomic::Ordering::Relaxed);
            metrics::STREAM_GENERATION_DURATION_SECONDS.observe(upstream_started.elapsed().as_secs_f64());
            metrics::STREAM_UPSTREAM_RESPONSE_BYTES.observe(upstream_bytes as f64);
            let mut upstream_completed = EvidenceEvent::new(
                trace_id, EvidenceSource::AiFirewall, EventCategory::Upstream,
                "upstream.stream.completed", EventOutcome::Completed,
            );
            upstream_completed.attributes.insert("delivery_mode".into(), serde_json::json!("progressive"));
            upstream_completed.attributes.insert("bytes".into(), serde_json::json!(upstream_bytes));
            let _ = evidence_sink.emit(upstream_completed).await;

            let finalization_task = tokio::spawn(async move {
                finalizer.finalize_progressive_response(&response, &finalization).await
            });
            match finalization_task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    terminal.fail_progressive("cache_finalization_error", false, first_client).await;
                    Err(error)?;
                }
                Err(error) => {
                    terminal.fail_progressive("cache_finalization_task_failed", false, first_client).await;
                    Err(io::Error::other(format!("progressive finalization task failed: {error}")))?;
                }
            }
            metrics::STREAM_COMPLETED_TOTAL.inc();
            metrics::STREAM_DURATION_SECONDS.observe(delivery_started.elapsed().as_secs_f64());
            terminal.terminal_emitted = true;
            let mut completed = EvidenceEvent::new(
                trace_id, EvidenceSource::AiFirewall, EventCategory::Request,
                "request.completed", EventOutcome::Completed,
            );
            completed.attributes.insert("stream".into(), serde_json::Value::Bool(true));
            completed.attributes.insert("delivery_mode".into(), serde_json::Value::String("progressive".into()));
            let _ = evidence_sink.emit(completed).await;
            yield Bytes::from_static(b"data: [DONE]\n\n");
        };
        metrics::STREAM_DELIVERY_SELECTED_TOTAL
            .with_label_values(&["progressive_upstream"])
            .inc();
        tracing::debug!(%trace_id, delivery_path = "progressive_upstream", "streaming execution path selected");
        Ok(Box::pin(stream.map(|result| {
            result.map_err(|error: io::Error| Box::new(error) as UpstreamStreamError)
        })))
    }

    async fn finalize_progressive_response(
        &self,
        response: &ChatCompletionResponse,
        context: &ProgressiveFinalization,
    ) -> Result<(), io::Error> {
        self.record_upstream_model_cost(response);
        if self.enforcement_mode.is_observe() {
            match context.evaluation_hit {
                Some(CACHE_TYPE_EXACT) => self.record_evaluation_exact_hit_savings(response),
                Some(CACHE_TYPE_SEMANTIC) => self.record_evaluation_semantic_hit_savings(
                    response,
                    context.evaluation_semantic_lookup_tokens,
                ),
                _ => {}
            }
        }

        if !context.store_after_upstream || context.bypass_store || !cacheable_response(response) {
            if !cacheable_response(response) {
                tracing::debug!(
                    "progressive upstream completion excluded from both caches by admission policy"
                );
            }
            return Ok(());
        }

        let observe_mode = self.enforcement_mode.is_observe();
        let mut evaluation_incomplete = context.evaluation_incomplete;
        if self.exact_cache_enabled && self.exact_cache_store_enabled {
            if observe_mode && !self.track_redis_runtime {
                evaluation_incomplete = true;
                self.record_evaluation_error("redis", "store");
                self.record_evaluation_shadow_store(CACHE_TYPE_EXACT, "error");
            } else {
                let raw = serde_json::to_string(response).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("response encode failed: {error}"),
                    )
                })?;
                match self.exact_cache.set(&context.exact_key, raw).await {
                    Ok(()) => {
                        self.set_redis_available(true);
                        if observe_mode {
                            self.record_evaluation_shadow_store(CACHE_TYPE_EXACT, "stored");
                        }
                    }
                    Err(error) => {
                        self.set_redis_available(false);
                        if observe_mode {
                            evaluation_incomplete = true;
                            self.record_evaluation_error("redis", "store");
                            self.record_evaluation_shadow_store(CACHE_TYPE_EXACT, "error");
                        }
                        tracing::warn!(error = %error, "progressive exact cache store failed; response already delivered (fail-open)");
                    }
                }
            }
        }

        if self.semantic_cache_enabled
            && self.semantic_cache_store_enabled
            && context.semantic_text.is_some()
        {
            if observe_mode && !self.track_qdrant_runtime {
                evaluation_incomplete = true;
                self.record_evaluation_error("qdrant", "store");
                self.record_evaluation_shadow_store(CACHE_TYPE_SEMANTIC, "error");
            } else {
                let text = context.semantic_text.as_deref().expect("checked above");
                match self
                    .semantic_cache
                    .store(
                        context.request.normalized_model(),
                        text,
                        response,
                        context
                            .semantic_compatibility_scope
                            .as_deref()
                            .expect("scope for eligible request"),
                        context.privacy_cache_scope.as_deref(),
                        context.privacy_placeholder_signature.as_deref(),
                    )
                    .await
                {
                    Ok(embedding_usage) => {
                        self.set_qdrant_available(true);
                        if observe_mode {
                            self.record_evaluation_shadow_store(CACHE_TYPE_SEMANTIC, "stored");
                        }
                        if let Some(usage) = embedding_usage {
                            self.record_embedding_overhead(
                                context.request.normalized_model(),
                                EMBEDDING_OPERATION_STORE,
                                usage.prompt_tokens,
                            );
                        }
                    }
                    Err(error) => {
                        self.set_qdrant_available(false);
                        self.record_semantic_skip("store_error");
                        if observe_mode {
                            evaluation_incomplete = true;
                            self.record_evaluation_error("qdrant", "store");
                            self.record_evaluation_shadow_store(CACHE_TYPE_SEMANTIC, "error");
                        }
                        tracing::warn!(error = %error, "progressive semantic cache store failed; response already delivered (fail-open)");
                    }
                }
            }
        }
        if observe_mode && evaluation_incomplete {
            tracing::debug!(
                "progressive shadow evaluation completed with non-blocking cache errors"
            );
        }
        Ok(())
    }

    async fn fetch_controlled_stream_response(
        &self,
        req: &ChatCompletionRequest,
        trace_id: uuid::Uuid,
        max_stream_upstream_bytes: usize,
    ) -> Result<ChatCompletionResponse, AppError> {
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
            endpoint_class: Some("chat_completions_stream".into()),
            response_status: None,
            latency_ms: None,
        });
        upstream_event
            .attributes
            .insert("stream".into(), serde_json::Value::Bool(true));
        upstream_event.attributes.insert(
            "delivery_mode".into(),
            serde_json::Value::String("controlled".into()),
        );
        self.emit(upstream_event).await;

        let _upstream_permit = self
            .upstream_limit
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                metrics::observe_backpressure_rejection("upstream");
                AppError::backpressure("upstream", "upstream concurrency limit reached")
            })?;

        let mut upstream_req = req.clone();
        upstream_req.stream = Some(true);
        let stream_options = upstream_req
            .extra
            .entry("stream_options".to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if !stream_options.is_object() {
            *stream_options = serde_json::Value::Object(serde_json::Map::new());
        }
        if let Some(options) = stream_options.as_object_mut() {
            options.insert("include_usage".to_string(), serde_json::Value::Bool(true));
        }

        let upstream_started = Instant::now();
        let upstream = match self.upstream.chat_completion_stream(&upstream_req).await {
            Ok(response) => response,
            Err(error) => {
                self.dependencies
                    .upstream_available
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                metrics::STREAM_UPSTREAM_ERRORS_TOTAL.inc();

                let mut failed = EvidenceEvent::new(
                    trace_id,
                    EvidenceSource::AiFirewall,
                    EventCategory::Upstream,
                    "upstream.request.failed",
                    EventOutcome::Failed,
                );
                failed.upstream = Some(UpstreamEvidence {
                    provider_type: self.upstream_metadata.provider_type.clone(),
                    provider_name: self.upstream_metadata.provider_name.clone(),
                    model: req.normalized_model().to_string(),
                    endpoint_class: Some("chat_completions_stream".into()),
                    response_status: None,
                    latency_ms: Some(upstream_started.elapsed().as_millis() as u64),
                });
                failed.decision = Some(DecisionEvidence {
                    action: "fail_request".into(),
                    reason_code: error.evidence_reason_code().into(),
                    rule_id: None,
                    severity: None,
                });
                self.emit(failed).await;
                return Err(error);
            }
        };

        let response_status = upstream.status.as_u16();
        let mut headers_received = EvidenceEvent::new(
            trace_id,
            EvidenceSource::AiFirewall,
            EventCategory::Upstream,
            "upstream.response.received",
            EventOutcome::Started,
        );
        headers_received.upstream = Some(UpstreamEvidence {
            provider_type: self.upstream_metadata.provider_type.clone(),
            provider_name: self.upstream_metadata.provider_name.clone(),
            model: req.normalized_model().to_string(),
            endpoint_class: Some("chat_completions_stream".into()),
            response_status: Some(response_status),
            latency_ms: Some(upstream_started.elapsed().as_millis() as u64),
        });
        headers_received
            .attributes
            .insert("phase".into(), serde_json::Value::String("headers".into()));
        headers_received
            .attributes
            .insert("stream".into(), serde_json::Value::Bool(true));
        self.emit(headers_received).await;

        let mut assembler =
            OpenAiStreamAssembler::new(req.normalized_model(), max_stream_upstream_bytes);
        let mut upstream_body = upstream.body;
        let mut first_byte_seen = false;
        let mut chunks = 0_u64;
        let mut bytes_total = 0_u64;
        let stream_idle_timeout = self.upstream.stream_idle_timeout();
        let stream_generation_timeout = self.upstream.max_stream_generation_duration();
        let generation_deadline = tokio::time::Instant::now() + stream_generation_timeout;

        loop {
            let now = tokio::time::Instant::now();
            let idle_deadline = now + stream_idle_timeout;
            let (next_deadline, timeout_class, timeout_message) = if generation_deadline
                <= idle_deadline
            {
                (
                        generation_deadline,
                        "stream_generation_timeout",
                        format!(
                            "upstream controlled stream exceeded the maximum generation duration of {} seconds before commit",
                            stream_generation_timeout.as_secs()
                        ),
                    )
            } else {
                (
                        idle_deadline,
                        "stream_idle_timeout",
                        format!(
                            "upstream controlled stream produced no body chunk for {} seconds before commit",
                            stream_idle_timeout.as_secs()
                        ),
                    )
            };

            let next_item = match tokio::time::timeout_at(next_deadline, upstream_body.next()).await
            {
                Ok(item) => item,
                Err(_) => {
                    if timeout_class == "stream_idle_timeout" {
                        self.dependencies
                            .upstream_available
                            .store(false, std::sync::atomic::Ordering::Relaxed);
                    }
                    metrics::UPSTREAM_TIMEOUTS_TOTAL.inc();
                    metrics::STREAM_UPSTREAM_ERRORS_TOTAL.inc();

                    self.emit_stream_failure(
                        trace_id,
                        req,
                        upstream.status.as_u16(),
                        upstream_started,
                        StreamProgress {
                            chunks,
                            bytes: bytes_total,
                        },
                        timeout_class,
                    )
                    .await;

                    return Err(AppError::upstream_kind(
                        UpstreamErrorKind::Timeout,
                        timeout_message,
                    ));
                }
            };

            let Some(item) = next_item else {
                break;
            };

            let bytes = match item {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.dependencies
                        .upstream_available
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    metrics::STREAM_UPSTREAM_ERRORS_TOTAL.inc();

                    let app_error = AppError::upstream_kind(
                        UpstreamErrorKind::Other,
                        format!("upstream stream body failed before commit: {error}"),
                    );
                    self.emit_stream_failure(
                        trace_id,
                        req,
                        upstream.status.as_u16(),
                        upstream_started,
                        StreamProgress {
                            chunks,
                            bytes: bytes_total,
                        },
                        "upstream_stream_error",
                    )
                    .await;
                    return Err(app_error);
                }
            };

            if !first_byte_seen {
                metrics::STREAM_UPSTREAM_TIME_TO_FIRST_BYTE_SECONDS
                    .observe(upstream_started.elapsed().as_secs_f64());
                first_byte_seen = true;
            }

            chunks = chunks.saturating_add(1);
            bytes_total = bytes_total.saturating_add(bytes.len() as u64);
            metrics::STREAM_UPSTREAM_CHUNKS_TOTAL.inc();
            metrics::STREAM_UPSTREAM_BYTES_TOTAL.inc_by(bytes.len() as u64);

            if let Err(error) = assembler.push_bytes(&bytes) {
                self.dependencies
                    .upstream_available
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                metrics::STREAM_UPSTREAM_ERRORS_TOTAL.inc();

                self.emit_stream_failure(
                    trace_id,
                    req,
                    upstream.status.as_u16(),
                    upstream_started,
                    StreamProgress {
                        chunks,
                        bytes: bytes_total,
                    },
                    "upstream_stream_error",
                )
                .await;

                return Err(AppError::upstream_kind(
                    UpstreamErrorKind::Other,
                    format!("invalid upstream streaming response: {error}"),
                ));
            }
        }

        let upstream_response_bytes = assembler.received_bytes();
        let response = match assembler.finish() {
            Ok(response) => response,
            Err(error) => {
                self.dependencies
                    .upstream_available
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                metrics::STREAM_UPSTREAM_ERRORS_TOTAL.inc();
                self.emit_stream_failure(
                    trace_id,
                    req,
                    upstream.status.as_u16(),
                    upstream_started,
                    StreamProgress {
                        chunks,
                        bytes: bytes_total,
                    },
                    "upstream_stream_error",
                )
                .await;

                return Err(AppError::upstream_kind(
                    UpstreamErrorKind::Other,
                    format!("invalid upstream streaming response: {error}"),
                ));
            }
        };

        self.dependencies
            .upstream_available
            .store(true, std::sync::atomic::Ordering::Relaxed);
        metrics::STREAM_GENERATION_DURATION_SECONDS
            .observe(upstream_started.elapsed().as_secs_f64());
        metrics::STREAM_UPSTREAM_RESPONSE_BYTES.observe(upstream_response_bytes as f64);

        let mut completed = EvidenceEvent::new(
            trace_id,
            EvidenceSource::AiFirewall,
            EventCategory::Upstream,
            "upstream.stream.completed",
            EventOutcome::Completed,
        );
        completed.upstream = Some(UpstreamEvidence {
            provider_type: self.upstream_metadata.provider_type.clone(),
            provider_name: self.upstream_metadata.provider_name.clone(),
            model: response.model.clone(),
            endpoint_class: Some("chat_completions_stream".into()),
            response_status: Some(response_status),
            latency_ms: Some(upstream_started.elapsed().as_millis() as u64),
        });
        completed
            .attributes
            .insert("chunks".into(), serde_json::Value::from(chunks));
        completed
            .attributes
            .insert("bytes".into(), serde_json::Value::from(bytes_total));
        completed.attributes.insert(
            "delivery_mode".into(),
            serde_json::Value::String("controlled".into()),
        );
        self.emit(completed).await;

        Ok(response)
    }

    async fn emit_stream_failure(
        &self,
        trace_id: uuid::Uuid,
        req: &ChatCompletionRequest,
        response_status: u16,
        started: Instant,
        progress: StreamProgress,
        error_class: &'static str,
    ) {
        let mut failed = EvidenceEvent::new(
            trace_id,
            EvidenceSource::AiFirewall,
            EventCategory::Upstream,
            "upstream.stream.failed",
            EventOutcome::Failed,
        );
        failed.upstream = Some(UpstreamEvidence {
            provider_type: self.upstream_metadata.provider_type.clone(),
            provider_name: self.upstream_metadata.provider_name.clone(),
            model: req.normalized_model().to_string(),
            endpoint_class: Some("chat_completions_stream".into()),
            response_status: Some(response_status),
            latency_ms: Some(started.elapsed().as_millis() as u64),
        });
        failed
            .attributes
            .insert("chunks".into(), serde_json::Value::from(progress.chunks));
        failed
            .attributes
            .insert("bytes".into(), serde_json::Value::from(progress.bytes));
        failed.attributes.insert(
            "error_class".into(),
            serde_json::Value::String(error_class.into()),
        );
        failed
            .attributes
            .insert("committed_to_client".into(), serde_json::Value::Bool(false));
        self.emit(failed).await;
    }

    async fn security_scan_response(
        &self,
        guard_context: &GuardContext,
        response: ChatCompletionResponse,
        trace_id: uuid::Uuid,
    ) -> Result<ChatCompletionResponse, AppError> {
        self.guard_orchestrator
            .before_response_restore(guard_context, response, trace_id)
            .await
    }

    async fn restore_guarded_response(
        &self,
        guard_context: &GuardContext,
        response: ChatCompletionResponse,
        trace_id: uuid::Uuid,
    ) -> Result<ChatCompletionResponse, AppError> {
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
        controlled_stream: bool,
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
        if controlled_stream {
            let selected_path = match delivery_path {
                "exact_cache" => "exact_cache",
                "semantic_cache" => "semantic_cache",
                "upstream" => "controlled_upstream",
                _ => "policy_completion",
            };
            metrics::STREAM_DELIVERY_SELECTED_TOTAL
                .with_label_values(&[selected_path])
                .inc();
            tracing::debug!(%trace_id, delivery_path = selected_path, "streaming execution path selected");
            event
                .attributes
                .insert("stream".into(), serde_json::Value::Bool(true));
            event.attributes.insert(
                "delivery_mode".into(),
                serde_json::Value::String("controlled".into()),
            );
        }
        event
    }

    async fn emit(&self, mut event: EvidenceEvent) {
        event.attributes.insert(
            "enforcement_mode".into(),
            serde_json::Value::String(self.enforcement_mode.as_str().into()),
        );
        if let Err(error) = self.evidence_sink.emit(event).await {
            tracing::warn!(error = %error, "failed to emit VCAL evidence event");
        }
    }

    fn effective_exact_cache_prefix(&self) -> &'static str {
        if self.enforcement_mode.is_observe() {
            "chatcmpl:eval:v1"
        } else {
            "chatcmpl:v1"
        }
    }

    fn annotate_cache_decision(
        &self,
        event: &mut EvidenceEvent,
        decision: &str,
        would_action: &str,
        applied_action: &str,
    ) {
        event.attributes.insert(
            "decision".into(),
            serde_json::Value::String(decision.to_string()),
        );
        event.attributes.insert(
            "would_action".into(),
            serde_json::Value::String(would_action.to_string()),
        );
        event.attributes.insert(
            "applied_action".into(),
            serde_json::Value::String(applied_action.to_string()),
        );
    }

    fn record_evaluation_cache_outcome(&self, cache_type: &'static str, result: &'static str) {
        metrics::EVALUATION_CACHE_OUTCOMES_TOTAL
            .with_label_values(&[cache_type, result])
            .inc();
    }

    fn record_evaluation_shadow_store(&self, cache_type: &'static str, result: &'static str) {
        metrics::EVALUATION_SHADOW_STORE_TOTAL
            .with_label_values(&[cache_type, result])
            .inc();
    }

    fn record_evaluation_error(&self, component: &'static str, operation: &'static str) {
        metrics::EVALUATION_ERRORS_TOTAL
            .with_label_values(&[component, operation])
            .inc();
    }

    fn record_evaluation_upstream_call_avoided(&self, cache_type: &'static str) {
        metrics::EVALUATION_UPSTREAM_CALLS_AVOIDED_TOTAL
            .with_label_values(&[cache_type])
            .inc();
    }

    fn record_evaluation_semantic_lookup_overhead(&self, model: &str, prompt_tokens: u32) {
        let embedding_cost =
            estimate_embedding_micro_usd(prompt_tokens, self.embedding_price.as_ref());
        metrics::EVALUATION_EMBEDDING_OVERHEAD_MICRO_USD_TOTAL
            .with_label_values(&[model, EMBEDDING_OPERATION_LOOKUP])
            .inc_by(embedding_cost);
    }

    fn record_evaluation_exact_hit_savings(&self, response: &ChatCompletionResponse) {
        let Some(usage) = &response.usage else {
            return;
        };

        metrics::EVALUATION_TOKENS_AVOIDED_TOTAL
            .with_label_values(&[response.model.as_str(), CACHE_TYPE_EXACT])
            .inc_by(usage.total_tokens as u64);

        let saved = estimate_micro_usd_saved(&response.model, usage, &self.model_prices);
        metrics::EVALUATION_GROSS_SAVED_MICRO_USD_TOTAL
            .with_label_values(&[response.model.as_str(), CACHE_TYPE_EXACT])
            .inc_by(saved);
        metrics::EVALUATION_NET_SAVED_MICRO_USD_TOTAL
            .with_label_values(&[response.model.as_str(), CACHE_TYPE_EXACT])
            .inc_by(saved);
    }

    fn record_evaluation_semantic_hit_savings(
        &self,
        response: &ChatCompletionResponse,
        embedding_prompt_tokens: u32,
    ) {
        let Some(usage) = &response.usage else {
            return;
        };

        metrics::EVALUATION_TOKENS_AVOIDED_TOTAL
            .with_label_values(&[response.model.as_str(), CACHE_TYPE_SEMANTIC])
            .inc_by(usage.total_tokens as u64);

        let gross_saved = estimate_micro_usd_saved(&response.model, usage, &self.model_prices);
        let embedding_cost =
            estimate_embedding_micro_usd(embedding_prompt_tokens, self.embedding_price.as_ref());
        let net_saved = gross_saved.saturating_sub(embedding_cost);

        metrics::EVALUATION_GROSS_SAVED_MICRO_USD_TOTAL
            .with_label_values(&[response.model.as_str(), CACHE_TYPE_SEMANTIC])
            .inc_by(gross_saved);
        metrics::EVALUATION_NET_SAVED_MICRO_USD_TOTAL
            .with_label_values(&[response.model.as_str(), CACHE_TYPE_SEMANTIC])
            .inc_by(net_saved);
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
        if req.extra.contains_key("tools") {
            return false;
        }

        if req.extra.contains_key("response_format") {
            return false;
        }

        if req
            .messages
            .iter()
            .any(|message| !message.content.is_string())
        {
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
        upstream::llm::{LlmUpstream, UpstreamStreamError, UpstreamStreamResponse},
    };
    use async_trait::async_trait;
    use serde_json::{json, Map, Value};
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
        time::Duration,
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
        last_lookup_privacy_cache_scope: Option<String>,
        last_store_privacy_cache_scope: Option<String>,
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
            _compatibility_scope: &str,
            privacy_cache_scope: Option<&str>,
            privacy_placeholder_signature: Option<&str>,
        ) -> anyhow::Result<Option<SemanticLookupHit>> {
            let mut state = self.state.lock().unwrap();
            state.lookup_calls += 1;
            state.last_lookup_privacy_cache_scope = privacy_cache_scope.map(ToOwned::to_owned);
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
            _compatibility_scope: &str,
            privacy_cache_scope: Option<&str>,
            privacy_placeholder_signature: Option<&str>,
        ) -> anyhow::Result<Option<EmbeddingUsage>> {
            let mut state = self.state.lock().unwrap();
            state.store_calls += 1;
            state.last_store_privacy_cache_scope = privacy_cache_scope.map(ToOwned::to_owned);
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
    struct ScopeAwareSemanticCacheState {
        entry: Option<(String, String, String, ChatCompletionResponse)>,
        lookup_calls: usize,
        store_calls: usize,
    }

    struct ScopeAwareSemanticCache {
        state: Arc<Mutex<ScopeAwareSemanticCacheState>>,
    }

    impl ScopeAwareSemanticCache {
        fn new() -> Self {
            Self {
                state: Arc::new(Mutex::new(ScopeAwareSemanticCacheState::default())),
            }
        }

        fn state(&self) -> Arc<Mutex<ScopeAwareSemanticCacheState>> {
            Arc::clone(&self.state)
        }
    }

    #[async_trait]
    impl SemanticCache for ScopeAwareSemanticCache {
        async fn lookup(
            &self,
            _model: &str,
            _normalized_prompt: &str,
            compatibility_scope: &str,
            privacy_cache_scope: Option<&str>,
            privacy_placeholder_signature: Option<&str>,
        ) -> anyhow::Result<Option<SemanticLookupHit>> {
            let mut state = self.state.lock().unwrap();
            state.lookup_calls += 1;
            let scope = privacy_cache_scope.unwrap_or("");
            let signature = privacy_placeholder_signature.unwrap_or("");

            Ok(state.entry.as_ref().and_then(
                |(entry_scope, entry_signature, entry_compatibility, response)| {
                    (entry_scope == scope
                        && entry_signature == signature
                        && entry_compatibility == compatibility_scope)
                        .then(|| SemanticLookupHit {
                            response: response.clone(),
                            embedding_usage: None,
                        })
                },
            ))
        }

        async fn store(
            &self,
            _model: &str,
            _normalized_prompt: &str,
            response: &ChatCompletionResponse,
            compatibility_scope: &str,
            privacy_cache_scope: Option<&str>,
            privacy_placeholder_signature: Option<&str>,
        ) -> anyhow::Result<Option<EmbeddingUsage>> {
            let mut state = self.state.lock().unwrap();
            state.store_calls += 1;
            state.entry = Some((
                privacy_cache_scope.unwrap_or("").to_string(),
                privacy_placeholder_signature.unwrap_or("").to_string(),
                compatibility_scope.to_string(),
                response.clone(),
            ));
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

        async fn chat_completion_stream(
            &self,
            req: &ChatCompletionRequest,
        ) -> Result<UpstreamStreamResponse, AppError> {
            {
                let mut state = self.state.lock().unwrap();
                state.call_count += 1;
                state.last_request = Some(req.clone());
            }

            let content = self
                .response
                .choices
                .first()
                .and_then(|choice| choice.message.content.as_str())
                .unwrap_or_default()
                .to_string();
            let mut payload = format!(
                "data: {}\n\n",
                serde_json::json!({
                    "id": self.response.id.clone(),
                    "object": "chat.completion.chunk",
                    "created": self.response.created,
                    "model": self.response.model.clone(),
                    "choices": [{
                        "index": 0,
                        "delta": {"role": "assistant", "content": content},
                        "finish_reason": "stop"
                    }]
                })
            );
            if let Some(usage) = &self.response.usage {
                payload.push_str(&format!(
                    "data: {}\n\n",
                    serde_json::json!({
                        "id": self.response.id.clone(),
                        "object": "chat.completion.chunk",
                        "created": self.response.created,
                        "model": self.response.model.clone(),
                        "choices": [],
                        "usage": usage
                    })
                ));
            }
            payload.push_str("data: [DONE]\n\n");
            let body =
                stream::once(async move { Ok::<Bytes, UpstreamStreamError>(Bytes::from(payload)) });

            Ok(UpstreamStreamResponse {
                status: reqwest::StatusCode::OK,
                body: Box::pin(body),
            })
        }
    }

    struct FailingStreamUpstream;

    #[async_trait]
    impl LlmUpstream for FailingStreamUpstream {
        async fn chat_completion(
            &self,
            _req: &ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, AppError> {
            Ok(response_with_content("unused"))
        }

        async fn chat_completion_stream(
            &self,
            _req: &ChatCompletionRequest,
        ) -> Result<UpstreamStreamResponse, AppError> {
            let partial = Bytes::from(
                r#"data: {"id":"partial","created":123,"model":"gpt-4o-mini-2024-07-18","choices":[{"index":0,"delta":{"role":"assistant","content":"must never reach client"},"finish_reason":null}]}

"#,
            );
            let error: UpstreamStreamError =
                Box::new(std::io::Error::other("intentional mid-stream failure"));
            let body = stream::iter(vec![Ok(partial), Err(error)]);

            Ok(UpstreamStreamResponse {
                status: reqwest::StatusCode::OK,
                body: Box::pin(body),
            })
        }
    }

    struct StallingStreamUpstream;

    #[async_trait]
    impl LlmUpstream for StallingStreamUpstream {
        fn stream_idle_timeout(&self) -> Duration {
            Duration::from_millis(20)
        }

        fn max_stream_generation_duration(&self) -> Duration {
            Duration::from_secs(1)
        }

        async fn chat_completion(
            &self,
            _req: &ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, AppError> {
            Ok(response_with_content("unused"))
        }

        async fn chat_completion_stream(
            &self,
            _req: &ChatCompletionRequest,
        ) -> Result<UpstreamStreamResponse, AppError> {
            let body = stream::pending::<Result<Bytes, UpstreamStreamError>>();
            Ok(UpstreamStreamResponse {
                status: reqwest::StatusCode::OK,
                body: Box::pin(body),
            })
        }
    }

    struct DripFeedStreamUpstream;

    #[async_trait]
    impl LlmUpstream for DripFeedStreamUpstream {
        fn stream_idle_timeout(&self) -> Duration {
            Duration::from_millis(50)
        }

        fn max_stream_generation_duration(&self) -> Duration {
            Duration::from_millis(35)
        }

        async fn chat_completion(
            &self,
            _req: &ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, AppError> {
            Ok(response_with_content("unused"))
        }

        async fn chat_completion_stream(
            &self,
            _req: &ChatCompletionRequest,
        ) -> Result<UpstreamStreamResponse, AppError> {
            let body = stream::unfold((), |_| async {
                tokio::time::sleep(Duration::from_millis(5)).await;
                Some((
                    Ok::<Bytes, UpstreamStreamError>(Bytes::from_static(b": keepalive\n\n")),
                    (),
                ))
            });
            Ok(UpstreamStreamResponse {
                status: reqwest::StatusCode::OK,
                body: Box::pin(body),
            })
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
                    privacy_cache_scope: Some("scope-test".to_string()),
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

    fn evaluation_exact_key_for(req: &ChatCompletionRequest) -> String {
        let normalized = normalize_chat_request(req).unwrap();
        format!("chatcmpl:eval:v1:{}", sha256_hex(&normalized))
    }

    fn build_observe_service(
        exact_cache: Arc<dyn ExactCache>,
        semantic_cache: Arc<dyn SemanticCache>,
        upstream: Arc<dyn LlmUpstream>,
        semantic_cache_enabled: bool,
    ) -> ChatService {
        let mut service = build_service(
            exact_cache,
            semantic_cache,
            upstream,
            semantic_cache_enabled,
        );
        service.enforcement_mode = AifEnforcementMode::Observe;
        service
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

    struct UsagePolicyCompletionGuard;

    #[async_trait]
    impl GuardOrchestrator for UsagePolicyCompletionGuard {
        async fn before_cache(
            &self,
            request: ChatCompletionRequest,
            _trace_id: uuid::Uuid,
        ) -> Result<GuardedRequest, AppError> {
            Ok(GuardedRequest {
                request,
                context: GuardContext {
                    usage_policy_block_message: Some(
                        "This request is outside approved business use.".to_string(),
                    ),
                    ..GuardContext::default()
                },
                cache_control: CacheControl {
                    bypass_lookup: true,
                    bypass_store: true,
                },
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

    struct SecurityRequestCompletionGuard;

    #[async_trait]
    impl GuardOrchestrator for SecurityRequestCompletionGuard {
        async fn before_cache(
            &self,
            request: ChatCompletionRequest,
            _trace_id: uuid::Uuid,
        ) -> Result<GuardedRequest, AppError> {
            Ok(GuardedRequest {
                request,
                context: GuardContext {
                    security_policy_block_message: Some(
                        "This interaction was blocked by security policy.".to_string(),
                    ),
                    ..GuardContext::default()
                },
                cache_control: CacheControl {
                    bypass_lookup: true,
                    bypass_store: true,
                },
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

    #[tokio::test]
    async fn security_request_completion_skips_cache_and_upstream() {
        let exact = FakeExactCache::new();
        let exact_state = exact.state();
        let semantic = FakeSemanticCache::new();
        let semantic_state = semantic.state();
        let upstream = FakeUpstream::new(response_with_content("must not be used"));
        let upstream_state = upstream.state();

        let service = ChatService::new_with_guards(
            Arc::new(exact),
            Arc::new(semantic),
            Arc::new(upstream),
            Arc::new(SecurityRequestCompletionGuard),
            ChatServiceSettings {
                semantic_cache_enabled: true,
                exact_cache_enabled: true,
                exact_cache_fail_open: true,
                exact_cache_store_enabled: true,
                semantic_cache_store_enabled: true,
                semantic_cache_fail_open: true,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 500,
            },
            model_prices(),
            None,
        );

        let response = service.handle(request()).await.unwrap();

        assert!(response.id.starts_with("chatcmpl-vcal-security-"));
        assert_eq!(
            response.choices[0].message.content,
            json!("This interaction was blocked by security policy.")
        );
        assert_eq!(upstream_state.lock().unwrap().call_count, 0);
        assert_eq!(exact_state.lock().unwrap().get_calls, 0);
        assert_eq!(exact_state.lock().unwrap().set_calls, 0);
        assert_eq!(semantic_state.lock().unwrap().lookup_calls, 0);
        assert_eq!(semantic_state.lock().unwrap().store_calls, 0);
    }

    #[tokio::test]
    async fn security_blocked_semantic_hit_does_not_warm_exact_cache() {
        let restore_state = Arc::new(Mutex::new(RestoreCallState::default()));

        let semantic_cache = FakeSemanticCache::with_lookup_result(SemanticLookupHit {
            response: response_with_content("blocked semantic response"),
            embedding_usage: None,
        });
        let semantic_state = semantic_cache.state();

        let exact_cache = FakeExactCache::new();
        let exact_state = exact_cache.state();

        let upstream = FakeUpstream::new(response_with_content("must not be called"));
        let upstream_state = upstream.state();

        let service = ChatService::new_with_guards(
            Arc::new(exact_cache),
            Arc::new(semantic_cache),
            Arc::new(upstream),
            Arc::new(SecurityResponseCompletionGuard {
                restore_state: Arc::clone(&restore_state),
            }),
            ChatServiceSettings {
                semantic_cache_enabled: true,
                exact_cache_enabled: true,
                exact_cache_fail_open: true,
                exact_cache_store_enabled: true,
                semantic_cache_store_enabled: true,
                semantic_cache_fail_open: true,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 500,
            },
            model_prices(),
            None,
        );

        let response = service.handle(request()).await.unwrap();

        assert!(response.id.starts_with("chatcmpl-vcal-security-"));
        assert_eq!(exact_state.lock().unwrap().set_calls, 0);
        assert_eq!(semantic_state.lock().unwrap().lookup_calls, 1);
        assert_eq!(upstream_state.lock().unwrap().call_count, 0);
        assert_eq!(restore_state.lock().unwrap().calls, 0);
    }

    #[derive(Default)]
    struct RestoreCallState {
        calls: usize,
    }

    struct SecurityResponseCompletionGuard {
        restore_state: Arc<Mutex<RestoreCallState>>,
    }

    #[async_trait]
    impl GuardOrchestrator for SecurityResponseCompletionGuard {
        async fn before_cache(
            &self,
            request: ChatCompletionRequest,
            _trace_id: uuid::Uuid,
        ) -> Result<GuardedRequest, AppError> {
            Ok(GuardedRequest {
                request,
                context: GuardContext::default(),
                cache_control: CacheControl::default(),
            })
        }

        async fn before_response_restore(
            &self,
            _context: &GuardContext,
            response: ChatCompletionResponse,
            trace_id: uuid::Uuid,
        ) -> Result<ChatCompletionResponse, AppError> {
            let mut replacement = response;
            replacement.id = format!("chatcmpl-vcal-security-{}", trace_id.simple());
            replacement.choices[0].message.content =
                json!("This interaction was blocked by security policy.");
            replacement.usage = Some(Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                extra: serde_json::Map::new(),
            });
            Ok(replacement)
        }

        async fn restore_response(
            &self,
            _context: &GuardContext,
            response: ChatCompletionResponse,
            _trace_id: uuid::Uuid,
        ) -> Result<ChatCompletionResponse, AppError> {
            self.restore_state.lock().unwrap().calls += 1;
            Ok(response)
        }
    }

    #[tokio::test]
    async fn security_response_completion_skips_privacy_restore_and_completes_trace() {
        let restore_state = Arc::new(Mutex::new(RestoreCallState::default()));
        let sink = RecordingEvidenceSink::default();
        let events = Arc::clone(&sink.events);
        let upstream = FakeUpstream::new(response_with_content("blocked upstream response"));

        let service = ChatService::new_with_guards_and_evidence(
            ChatServiceDeps {
                exact_cache: Arc::new(FakeExactCache::new()),
                semantic_cache: Arc::new(FakeSemanticCache::new()),
                upstream: Arc::new(upstream),
                guard_orchestrator: Arc::new(SecurityResponseCompletionGuard {
                    restore_state: Arc::clone(&restore_state),
                }),
                evidence_sink: Arc::new(sink),
                upstream_metadata: UpstreamMetadata {
                    provider_type: "test".into(),
                    provider_name: "test".into(),
                },
                dependencies: DependencyState::new(true, true, true),
                enforcement_mode: AifEnforcementMode::Enforce,
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
        let response = service
            .handle_with_evidence(request(), CacheControl::default(), trace_id)
            .await
            .unwrap();

        assert!(response.id.starts_with("chatcmpl-vcal-security-"));
        assert_eq!(
            response.choices[0].message.content,
            json!("This interaction was blocked by security policy.")
        );
        assert_eq!(
            restore_state.lock().unwrap().calls,
            0,
            "Privacy restore must be skipped for the safe Security Guard replacement"
        );

        let events = events.lock().unwrap();
        let completed = events
            .iter()
            .find(|event| event.trace_id == trace_id && event.event_type == "request.completed")
            .expect("Security replacement should complete the request trace");
        assert_eq!(
            completed.attributes.get("delivery_path"),
            Some(&serde_json::Value::String("security_policy".to_string()))
        );
        assert!(!events
            .iter()
            .any(|event| { event.trace_id == trace_id && event.event_type == "request.failed" }));
    }

    #[tokio::test]
    async fn usage_policy_completion_skips_cache_and_upstream_and_completes_trace() {
        let exact = FakeExactCache::new();
        let exact_state = exact.state();
        let semantic = FakeSemanticCache::new();
        let semantic_state = semantic.state();
        let upstream = FakeUpstream::new(response_with_content("must not be used"));
        let upstream_state = upstream.state();
        let sink = RecordingEvidenceSink::default();
        let events = Arc::clone(&sink.events);

        let service = ChatService::new_with_guards_and_evidence(
            ChatServiceDeps {
                exact_cache: Arc::new(exact),
                semantic_cache: Arc::new(semantic),
                upstream: Arc::new(upstream),
                guard_orchestrator: Arc::new(UsagePolicyCompletionGuard),
                evidence_sink: Arc::new(sink),
                upstream_metadata: UpstreamMetadata {
                    provider_type: "test".into(),
                    provider_name: "test".into(),
                },
                dependencies: DependencyState::new(true, true, true),
                enforcement_mode: AifEnforcementMode::Enforce,
            },
            ChatServiceSettings {
                semantic_cache_enabled: true,
                exact_cache_enabled: true,
                exact_cache_fail_open: true,
                exact_cache_store_enabled: true,
                semantic_cache_store_enabled: true,
                semantic_cache_fail_open: true,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 500,
            },
            model_prices(),
            None,
        );

        let trace_id = uuid::Uuid::new_v4();
        let response = service
            .handle_with_evidence(request(), CacheControl::default(), trace_id)
            .await
            .expect("Usage policy completion should be returned successfully");

        assert!(response.id.starts_with("chatcmpl-vcal-policy-"));
        assert_eq!(response.object, "chat.completion");
        assert_eq!(response.model, "gpt-4o-mini-2024-07-18");
        assert_eq!(
            response.choices[0].message.content,
            json!("This request is outside approved business use.")
        );
        assert_eq!(response.choices[0].finish_reason.as_deref(), Some("stop"));
        let usage = response
            .usage
            .as_ref()
            .expect("synthetic response should include usage");
        assert_eq!(usage.prompt_tokens, 0);
        assert_eq!(usage.completion_tokens, 0);
        assert_eq!(usage.total_tokens, 0);

        assert_eq!(upstream_state.lock().unwrap().call_count, 0);
        assert_eq!(exact_state.lock().unwrap().get_calls, 0);
        assert_eq!(exact_state.lock().unwrap().set_calls, 0);
        assert_eq!(semantic_state.lock().unwrap().lookup_calls, 0);
        assert_eq!(semantic_state.lock().unwrap().store_calls, 0);

        let events = events.lock().unwrap();
        let completed = events
            .iter()
            .find(|event| event.trace_id == trace_id && event.event_type == "request.completed")
            .expect("synthetic policy response should complete the request trace");
        assert_eq!(
            completed.attributes.get("delivery_path"),
            Some(&serde_json::Value::String("usage_policy".to_string()))
        );
        assert!(!events
            .iter()
            .any(|event| { event.trace_id == trace_id && event.event_type == "request.failed" }));
    }

    struct UserMappingGuard;

    fn extract_test_email(request: &ChatCompletionRequest) -> String {
        request.messages[0]
            .content
            .as_str()
            .unwrap_or_default()
            .split_whitespace()
            .find(|part| part.contains('@'))
            .unwrap_or_default()
            .trim_matches(|c: char| c == ',' || c == '.')
            .to_string()
    }

    fn restore_test_email(
        context: &GuardContext,
        mut response: ChatCompletionResponse,
    ) -> ChatCompletionResponse {
        if let Some(email) = context.privacy_mapping_id.as_deref() {
            let content = response.choices[0]
                .message
                .content
                .as_str()
                .unwrap_or_default()
                .replace("[EMAIL_1]", email);
            response.choices[0].message.content = json!(content);
        }
        response
    }

    #[async_trait]
    impl GuardOrchestrator for UserMappingGuard {
        async fn before_cache(
            &self,
            mut request: ChatCompletionRequest,
            _trace_id: uuid::Uuid,
        ) -> Result<GuardedRequest, AppError> {
            let email = extract_test_email(&request);
            request.messages[0].content = json!("Email [EMAIL_1]");

            Ok(GuardedRequest {
                request,
                context: GuardContext {
                    privacy_mapping_id: Some(email),
                    privacy_tenant_id: Some("tenant-a".to_string()),
                    privacy_policy_id: Some("policy-a".to_string()),
                    privacy_policy_version: Some("3".to_string()),
                    privacy_policy_hash: Some("sha256:policy-a-v3".to_string()),
                    privacy_cache_scope: Some("scope-tenant-a-policy-a-v3".to_string()),
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
            response: ChatCompletionResponse,
            _trace_id: uuid::Uuid,
        ) -> Result<ChatCompletionResponse, AppError> {
            Ok(restore_test_email(context, response))
        }
    }

    struct ScopedUserMappingGuard;

    #[async_trait]
    impl GuardOrchestrator for ScopedUserMappingGuard {
        async fn before_cache(
            &self,
            mut request: ChatCompletionRequest,
            _trace_id: uuid::Uuid,
        ) -> Result<GuardedRequest, AppError> {
            let email = extract_test_email(&request);
            let tenant = request
                .extra
                .remove("__test_privacy_tenant")
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| "tenant-a".to_string());
            let policy = request
                .extra
                .remove("__test_privacy_policy")
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| "policy-a".to_string());

            request.messages[0].content = json!("Email [EMAIL_1]");

            Ok(GuardedRequest {
                request,
                context: GuardContext {
                    privacy_mapping_id: Some(email),
                    privacy_tenant_id: Some(tenant.clone()),
                    privacy_policy_id: Some(policy.clone()),
                    privacy_policy_version: Some("3".to_string()),
                    privacy_policy_hash: Some(format!("sha256:{policy}-v3")),
                    privacy_cache_scope: Some(format!("scope:{tenant}:{policy}:3")),
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
            response: ChatCompletionResponse,
            _trace_id: uuid::Uuid,
        ) -> Result<ChatCompletionResponse, AppError> {
            Ok(restore_test_email(context, response))
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
    async fn observe_mode_never_fails_request_on_redis_error_even_when_fail_open_is_false() {
        let upstream = FakeUpstream::new(response_with_content("upstream ok"));
        let upstream_state = upstream.state();
        let mut service = ChatService::new(
            Arc::new(FailingExactCache),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(upstream),
            ChatServiceSettings {
                semantic_cache_enabled: false,
                exact_cache_enabled: true,
                exact_cache_fail_open: false,
                exact_cache_store_enabled: true,
                semantic_cache_store_enabled: false,
                semantic_cache_fail_open: false,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 500,
            },
            model_prices(),
            None,
        );
        service.enforcement_mode = AifEnforcementMode::Observe;

        let result = service.handle(request()).await;

        assert!(result.is_ok());
        assert_eq!(upstream_state.lock().unwrap().call_count, 1);
    }

    #[tokio::test]
    async fn observe_mode_never_fails_request_on_semantic_error_even_when_fail_open_is_false() {
        let upstream = FakeUpstream::new(response_with_content("upstream ok"));
        let upstream_state = upstream.state();
        let mut service = ChatService::new(
            Arc::new(FakeExactCache::new()),
            Arc::new(FakeSemanticCache::with_lookup_error(
                "simulated Qdrant outage",
            )),
            Arc::new(upstream),
            ChatServiceSettings {
                semantic_cache_enabled: true,
                exact_cache_enabled: false,
                exact_cache_fail_open: false,
                exact_cache_store_enabled: false,
                semantic_cache_store_enabled: true,
                semantic_cache_fail_open: false,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 500,
            },
            model_prices(),
            None,
        );
        service.enforcement_mode = AifEnforcementMode::Observe;

        let result = service.handle(request()).await;

        assert!(result.is_ok());
        assert_eq!(upstream_state.lock().unwrap().call_count, 1);
    }

    #[tokio::test]
    async fn observe_cache_bypass_performs_no_shadow_lookup_or_store() {
        let req = request();
        let exact_cache = FakeExactCache::with_entry(
            evaluation_exact_key_for(&req),
            serde_json::to_string(&response_with_content("shadow hit")).unwrap(),
        );
        let exact_state = exact_cache.state();
        let semantic_cache = FakeSemanticCache::with_lookup_result(SemanticLookupHit {
            response: response_with_content("semantic shadow hit"),
            embedding_usage: None,
        });
        let semantic_state = semantic_cache.state();
        let upstream = FakeUpstream::new(response_with_content("live upstream"));
        let upstream_state = upstream.state();
        let service = build_observe_service(
            Arc::new(exact_cache),
            Arc::new(semantic_cache),
            Arc::new(upstream),
            true,
        );

        let response = service
            .handle_with_evidence(
                req,
                CacheControl {
                    bypass_lookup: true,
                    bypass_store: true,
                },
                uuid::Uuid::new_v4(),
            )
            .await
            .unwrap();

        assert_eq!(response.id, "upstream-response");
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
    async fn observe_evidence_records_would_action_and_applied_action() {
        let req = request();
        let cached = response_with_usage("shadow-exact-hit", 1000, 500);
        let sink = RecordingEvidenceSink::default();
        let events = sink.events.clone();
        let service = ChatService::new_with_guards_and_evidence(
            ChatServiceDeps {
                exact_cache: Arc::new(FakeExactCache::with_entry(
                    evaluation_exact_key_for(&req),
                    serde_json::to_string(&cached).unwrap(),
                )),
                semantic_cache: Arc::new(FakeSemanticCache::new()),
                upstream: Arc::new(FakeUpstream::new(response_with_content("live upstream"))),
                guard_orchestrator: Arc::new(crate::guards::NoopGuardOrchestrator),
                evidence_sink: Arc::new(sink),
                upstream_metadata: UpstreamMetadata {
                    provider_type: "test".into(),
                    provider_name: "test".into(),
                },
                dependencies: DependencyState::new(true, true, true),
                enforcement_mode: AifEnforcementMode::Observe,
            },
            ChatServiceSettings {
                semantic_cache_enabled: true,
                exact_cache_enabled: true,
                exact_cache_fail_open: false,
                exact_cache_store_enabled: true,
                semantic_cache_store_enabled: true,
                semantic_cache_fail_open: false,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 500,
            },
            model_prices(),
            None,
        );

        let response = service.handle(req).await.unwrap();
        assert_eq!(response.id, "upstream-response");

        let events = events.lock().unwrap();
        assert!(events
            .iter()
            .all(|event| { event.attributes.get("enforcement_mode") == Some(&json!("observe")) }));
        let exact_hit = events
            .iter()
            .find(|event| event.event_type == "cache.exact.lookup")
            .expect("exact lookup evidence must be present");
        assert_eq!(exact_hit.outcome, EventOutcome::Hit);
        assert_eq!(
            exact_hit.attributes.get("decision"),
            Some(&json!("exact_hit"))
        );
        assert_eq!(
            exact_hit.attributes.get("would_action"),
            Some(&json!("serve_exact_cache"))
        );
        assert_eq!(
            exact_hit.attributes.get("applied_action"),
            Some(&json!("call_upstream"))
        );
        assert_eq!(
            exact_hit.cache.as_ref().map(|cache| cache.upstream_called),
            Some(true)
        );
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
                enforcement_mode: AifEnforcementMode::Enforce,
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
                enforcement_mode: AifEnforcementMode::Enforce,
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
    async fn same_scope_exact_cache_restores_only_current_request_pii_and_stores_placeholders() {
        let exact = FakeExactCache::new();
        let exact_state = exact.state();
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

        let exact = exact_state.lock().unwrap();
        assert_eq!(exact.entries.len(), 1);
        let cached = exact.entries.values().next().expect("cached response");
        assert!(cached.contains("[EMAIL_1]"));
        assert!(!cached.contains("alice@example.com"));
        assert!(!cached.contains("bob@example.com"));
    }

    #[tokio::test]
    async fn exact_cache_isolated_across_privacy_tenants() {
        let exact = FakeExactCache::new();
        let exact_state = exact.state();
        let upstream = FakeUpstream::new(response_with_content("Response for [EMAIL_1]"));
        let upstream_state = upstream.state();
        let service = ChatService::new_with_guards(
            Arc::new(exact),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(upstream),
            Arc::new(ScopedUserMappingGuard),
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

        let mut tenant_a = request();
        tenant_a.messages[0].content = json!("Email alice@example.com");
        tenant_a
            .extra
            .insert("__test_privacy_tenant".into(), json!("tenant-a"));
        service.handle(tenant_a).await.unwrap();

        let mut tenant_b = request();
        tenant_b.messages[0].content = json!("Email bob@example.com");
        tenant_b
            .extra
            .insert("__test_privacy_tenant".into(), json!("tenant-b"));
        service.handle(tenant_b).await.unwrap();

        assert_eq!(upstream_state.lock().unwrap().call_count, 2);
        assert_eq!(exact_state.lock().unwrap().entries.len(), 2);
    }

    #[tokio::test]
    async fn exact_cache_isolated_across_privacy_policies() {
        let exact = FakeExactCache::new();
        let exact_state = exact.state();
        let upstream = FakeUpstream::new(response_with_content("Response for [EMAIL_1]"));
        let upstream_state = upstream.state();
        let service = ChatService::new_with_guards(
            Arc::new(exact),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(upstream),
            Arc::new(ScopedUserMappingGuard),
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

        let mut policy_a = request();
        policy_a.messages[0].content = json!("Email alice@example.com");
        policy_a
            .extra
            .insert("__test_privacy_policy".into(), json!("policy-a"));
        service.handle(policy_a).await.unwrap();

        let mut policy_b = request();
        policy_b.messages[0].content = json!("Email bob@example.com");
        policy_b
            .extra
            .insert("__test_privacy_policy".into(), json!("policy-b"));
        service.handle(policy_b).await.unwrap();

        assert_eq!(upstream_state.lock().unwrap().call_count, 2);
        assert_eq!(exact_state.lock().unwrap().entries.len(), 2);
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
    async fn observe_exact_hit_returns_live_upstream_response_and_skips_semantic_evaluation() {
        let req = request();
        let cached = response_with_usage("shadow-exact-hit", 1000, 500);
        let upstream_response = response_with_usage("live-upstream", 20, 10);

        let exact_cache = FakeExactCache::with_entry(
            evaluation_exact_key_for(&req),
            serde_json::to_string(&cached).unwrap(),
        );
        let exact_state = exact_cache.state();
        let semantic_cache = FakeSemanticCache::new();
        let semantic_state = semantic_cache.state();
        let upstream = FakeUpstream::new(upstream_response);
        let upstream_state = upstream.state();

        let service = build_observe_service(
            Arc::new(exact_cache),
            Arc::new(semantic_cache),
            Arc::new(upstream),
            true,
        );

        let result = service.handle(req).await.unwrap();

        assert_eq!(result.id, "live-upstream");
        assert_eq!(upstream_state.lock().unwrap().call_count, 1);
        assert_eq!(semantic_state.lock().unwrap().lookup_calls, 0);
        assert_eq!(semantic_state.lock().unwrap().store_calls, 0);
        let exact = exact_state.lock().unwrap();
        assert_eq!(exact.get_calls, 1);
        assert_eq!(exact.set_calls, 0);
    }

    #[tokio::test]
    async fn observe_exact_hit_estimates_savings_from_the_live_upstream_call() {
        const MODEL: &str = "eval-accounting-test-model-v08";

        let req = request();
        let cached = response_with_usage("shadow-exact-hit", 10_000, 5_000);
        let mut live = response_with_usage("live-upstream", 20, 10);
        live.model = MODEL.to_string();

        let mut prices = HashMap::new();
        prices.insert(
            MODEL.to_string(),
            ModelPrice {
                input_usd_per_1m_tokens: 1.0,
                output_usd_per_1m_tokens: 2.0,
            },
        );

        let gross = metrics::EVALUATION_GROSS_SAVED_MICRO_USD_TOTAL
            .with_label_values(&[MODEL, CACHE_TYPE_EXACT]);
        let tokens =
            metrics::EVALUATION_TOKENS_AVOIDED_TOTAL.with_label_values(&[MODEL, CACHE_TYPE_EXACT]);
        let gross_before = gross.get();
        let tokens_before = tokens.get();

        let service = ChatService::new_with_guards_and_evidence(
            ChatServiceDeps {
                exact_cache: Arc::new(FakeExactCache::with_entry(
                    evaluation_exact_key_for(&req),
                    serde_json::to_string(&cached).unwrap(),
                )),
                semantic_cache: Arc::new(FakeSemanticCache::new()),
                upstream: Arc::new(FakeUpstream::new(live)),
                guard_orchestrator: Arc::new(crate::guards::NoopGuardOrchestrator),
                evidence_sink: Arc::new(NoopEvidenceSink),
                upstream_metadata: UpstreamMetadata {
                    provider_type: "test".into(),
                    provider_name: "test".into(),
                },
                dependencies: DependencyState::new(true, true, true),
                enforcement_mode: AifEnforcementMode::Observe,
            },
            ChatServiceSettings {
                semantic_cache_enabled: false,
                exact_cache_enabled: true,
                exact_cache_fail_open: false,
                exact_cache_store_enabled: true,
                semantic_cache_store_enabled: false,
                semantic_cache_fail_open: false,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 500,
            },
            prices,
            None,
        );

        let response = service.handle(req).await.unwrap();

        assert_eq!(response.id, "live-upstream");
        assert_eq!(gross.get() - gross_before, 40);
        assert_eq!(tokens.get() - tokens_before, 30);
    }

    #[tokio::test]
    async fn observe_semantic_hit_returns_live_upstream_and_warms_only_shadow_exact_cache() {
        let req = request();
        let semantic_response = response_with_usage("shadow-semantic-hit", 1000, 500);
        let semantic_cache = FakeSemanticCache::with_lookup_result(SemanticLookupHit {
            response: semantic_response,
            embedding_usage: Some(EmbeddingUsage {
                prompt_tokens: 1000,
                total_tokens: 1000,
            }),
        });
        let semantic_state = semantic_cache.state();
        let exact_cache = FakeExactCache::new();
        let exact_state = exact_cache.state();
        let upstream = FakeUpstream::new(response_with_usage("live-upstream", 20, 10));
        let upstream_state = upstream.state();

        let service = build_observe_service(
            Arc::new(exact_cache),
            Arc::new(semantic_cache),
            Arc::new(upstream),
            true,
        );

        let result = service.handle(req.clone()).await.unwrap();

        assert_eq!(result.id, "live-upstream");
        assert_eq!(upstream_state.lock().unwrap().call_count, 1);
        let semantic = semantic_state.lock().unwrap();
        assert_eq!(semantic.lookup_calls, 1);
        assert_eq!(semantic.store_calls, 0);
        drop(semantic);
        let exact = exact_state.lock().unwrap();
        assert_eq!(exact.get_calls, 1);
        assert_eq!(exact.set_calls, 1);
        assert!(exact.entries.contains_key(&evaluation_exact_key_for(&req)));
    }

    #[tokio::test]
    async fn observe_miss_returns_live_upstream_and_populates_shadow_exact_and_semantic_state() {
        let req = request();
        let exact_cache = FakeExactCache::new();
        let exact_state = exact_cache.state();
        let semantic_cache = FakeSemanticCache::new();
        let semantic_state = semantic_cache.state();
        let upstream = FakeUpstream::new(response_with_usage("live-upstream", 20, 10));
        let upstream_state = upstream.state();

        let service = build_observe_service(
            Arc::new(exact_cache),
            Arc::new(semantic_cache),
            Arc::new(upstream),
            true,
        );

        let result = service.handle(req.clone()).await.unwrap();

        assert_eq!(result.id, "live-upstream");
        assert_eq!(upstream_state.lock().unwrap().call_count, 1);
        let exact = exact_state.lock().unwrap();
        assert_eq!(exact.get_calls, 1);
        assert_eq!(exact.set_calls, 1);
        assert!(exact.entries.contains_key(&evaluation_exact_key_for(&req)));
        drop(exact);
        let semantic = semantic_state.lock().unwrap();
        assert_eq!(semantic.lookup_calls, 1);
        assert_eq!(semantic.store_calls, 1);
        assert_eq!(
            semantic.last_store_response.as_ref().unwrap().id,
            "live-upstream"
        );
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
    async fn length_limited_upstream_is_not_cached_or_promoted() {
        let mut req = request();
        req.max_tokens = Some(160);
        let mut truncated = response_with_content("partial reasoning");
        truncated.choices[0].finish_reason = Some("length".into());
        let exact = FakeExactCache::new();
        let exact_state = exact.state();
        let semantic = ScopeAwareSemanticCache::new();
        let semantic_state = semantic.state();
        let upstream = FakeUpstream::new(truncated);
        let upstream_state = upstream.state();
        let service = build_service(
            Arc::new(exact),
            Arc::new(semantic),
            Arc::new(upstream),
            true,
        );

        for prompt_max in [160, 160, 1024, 1024] {
            let mut next = req.clone();
            next.max_tokens = Some(prompt_max);
            let response = service.handle(next).await.unwrap();
            assert_eq!(response.choices[0].finish_reason.as_deref(), Some("length"));
        }
        assert_eq!(upstream_state.lock().unwrap().call_count, 4);
        assert_eq!(exact_state.lock().unwrap().set_calls, 0);
        assert_eq!(semantic_state.lock().unwrap().store_calls, 0);
    }

    #[tokio::test]
    async fn semantic_scope_prevents_cross_limit_reuse_and_allows_paraphrases() {
        let mut req = request();
        req.max_tokens = Some(160);
        let exact = FakeExactCache::new();
        let exact_state = exact.state();
        let semantic = ScopeAwareSemanticCache::new();
        let upstream = FakeUpstream::new(response_with_content("Complete answer"));
        let upstream_state = upstream.state();
        let service = build_service(
            Arc::new(exact),
            Arc::new(semantic),
            Arc::new(upstream),
            true,
        );

        // A complete response may populate both caches.
        assert_eq!(
            service.handle(req.clone()).await.unwrap().id,
            "upstream-response"
        );
        // An eligible paraphrase uses the semantic result and warms the exact cache.
        let mut paraphrase = req.clone();
        paraphrase.messages[0].content = json!("What is the account recovery process?");
        assert_eq!(
            service.handle(paraphrase.clone()).await.unwrap().id,
            "upstream-response"
        );
        assert_eq!(upstream_state.lock().unwrap().call_count, 1);
        assert_eq!(
            service.handle(paraphrase).await.unwrap().id,
            "upstream-response"
        );
        assert_eq!(upstream_state.lock().unwrap().call_count, 1);
        assert_eq!(exact_state.lock().unwrap().set_calls, 2);

        // The same semantic text with a different generation limit is a miss.
        let mut higher_limit = req;
        higher_limit.max_tokens = Some(1024);
        assert_eq!(
            service.handle(higher_limit).await.unwrap().id,
            "upstream-response"
        );
        assert_eq!(upstream_state.lock().unwrap().call_count, 2);
    }

    #[tokio::test]
    async fn legacy_length_limited_exact_cache_entry_is_ignored_on_read() {
        let req = request();
        let mut truncated = response_with_content("partial answer");
        truncated.choices[0].finish_reason = Some("length".into());
        let exact = FakeExactCache::with_entry(
            exact_key_for(&req),
            serde_json::to_string(&truncated).unwrap(),
        );
        let exact_state = exact.state();
        let upstream = FakeUpstream::new(response_with_content("Fresh complete answer"));
        let upstream_state = upstream.state();
        let service = build_service(
            Arc::new(exact),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(upstream),
            false,
        );
        let result = service.handle(req).await.unwrap();
        assert_eq!(
            result.choices[0].message.content,
            json!("Fresh complete answer")
        );
        assert_eq!(upstream_state.lock().unwrap().call_count, 1);
        assert_eq!(exact_state.lock().unwrap().set_calls, 1);
    }

    #[tokio::test]
    async fn invalid_semantic_hit_is_not_served_or_warmed_into_exact_cache() {
        let req = request();
        let mut truncated = response_with_content("partial answer");
        truncated.choices[0].finish_reason = Some("length".into());
        let semantic = FakeSemanticCache::with_lookup_result(SemanticLookupHit {
            response: truncated,
            embedding_usage: None,
        });
        let exact = FakeExactCache::new();
        let exact_state = exact.state();
        let upstream = FakeUpstream::new(response_with_content("Valid answer"));
        let upstream_state = upstream.state();
        let service = build_service(
            Arc::new(exact),
            Arc::new(semantic),
            Arc::new(upstream),
            true,
        );
        assert_eq!(
            service.handle(req).await.unwrap().choices[0]
                .message
                .content,
            json!("Valid answer")
        );
        assert_eq!(upstream_state.lock().unwrap().call_count, 1);
        assert_eq!(
            exact_state.lock().unwrap().set_calls,
            1,
            "only fresh valid upstream result may be stored"
        );
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
    async fn semantic_cache_does_not_reuse_entries_across_privacy_scopes() {
        let semantic_cache = ScopeAwareSemanticCache::new();
        let semantic_state = semantic_cache.state();
        let upstream = FakeUpstream::new(response_with_content("Response for [EMAIL_1]"));
        let upstream_state = upstream.state();
        let service = ChatService::new_with_guards(
            Arc::new(FakeExactCache::new()),
            Arc::new(semantic_cache),
            Arc::new(upstream),
            Arc::new(ScopedUserMappingGuard),
            ChatServiceSettings {
                semantic_cache_enabled: true,
                exact_cache_enabled: false,
                exact_cache_fail_open: true,
                exact_cache_store_enabled: false,
                semantic_cache_store_enabled: true,
                semantic_cache_fail_open: true,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 500,
            },
            model_prices(),
            None,
        );

        let mut tenant_a = request();
        tenant_a.messages[0].content = json!("Email alice@example.com");
        tenant_a
            .extra
            .insert("__test_privacy_tenant".into(), json!("tenant-a"));
        let first = service.handle(tenant_a).await.unwrap();
        assert_eq!(
            first.choices[0].message.content,
            json!("Response for alice@example.com")
        );

        let mut tenant_b = request();
        tenant_b.messages[0].content = json!("Email bob@example.com");
        tenant_b
            .extra
            .insert("__test_privacy_tenant".into(), json!("tenant-b"));
        let second = service.handle(tenant_b).await.unwrap();
        assert_eq!(
            second.choices[0].message.content,
            json!("Response for bob@example.com")
        );

        assert_eq!(
            upstream_state.lock().unwrap().call_count,
            2,
            "tenant-b must not receive tenant-a's semantic entry"
        );
        let semantic = semantic_state.lock().unwrap();
        assert_eq!(semantic.lookup_calls, 2);
        assert_eq!(semantic.store_calls, 2);
    }

    #[tokio::test]
    async fn guard_privacy_scope_and_placeholder_signature_are_passed_to_semantic_cache() {
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
            semantic.last_lookup_privacy_cache_scope.as_deref(),
            Some("scope-test")
        );
        assert_eq!(
            semantic.last_store_privacy_cache_scope.as_deref(),
            Some("scope-test")
        );
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
    async fn controlled_streaming_uses_caches_and_replays_canonical_response() {
        let mut req = request();
        req.stream = Some(true);
        req.extra
            .insert("stream_options".to_string(), json!({"include_usage": true}));

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

        for _ in 0..2 {
            let streamed = service
                .handle_stream_with_evidence(
                    req.clone(),
                    CacheControl::default(),
                    uuid::Uuid::new_v4(),
                    crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                    false,
                )
                .await
                .unwrap();
            assert_eq!(streamed.content_type, "text/event-stream");

            let mut body = streamed.body;
            let mut received = Vec::new();
            while let Some(item) = body.next().await {
                received.extend_from_slice(&item.unwrap());
            }
            let received = String::from_utf8(received).unwrap();
            assert!(received.contains("data: "));
            assert!(received.contains("Use the reset link on the login page."));
            assert!(received.contains("data: [DONE]"));
        }

        {
            let state = upstream_state.lock().unwrap();
            assert_eq!(
                state.call_count, 1,
                "second controlled stream should be served from exact cache"
            );
            assert_eq!(
                state
                    .last_request
                    .as_ref()
                    .and_then(|request| request.extra.get("stream_options"))
                    .and_then(Value::as_object)
                    .and_then(|options| options.get("include_usage"))
                    .and_then(Value::as_bool),
                Some(true),
                "controlled streaming should request usage internally for accounting"
            );
        }

        let mut buffered_req = req.clone();
        buffered_req.stream = Some(false);
        buffered_req.extra.remove("stream_options");
        let buffered = service
            .handle_with_evidence(buffered_req, CacheControl::default(), uuid::Uuid::new_v4())
            .await
            .unwrap();
        assert_eq!(
            buffered.choices[0].message.content,
            json!("Use the reset link on the login page.")
        );
        assert_eq!(
            upstream_state.lock().unwrap().call_count,
            1,
            "streaming and non-streaming delivery should share the exact cache"
        );

        assert_eq!(exact_state.lock().unwrap().get_calls, 3);
        assert_eq!(exact_state.lock().unwrap().set_calls, 1);
        assert_eq!(semantic_state.lock().unwrap().lookup_calls, 1);
        assert_eq!(semantic_state.lock().unwrap().store_calls, 1);
    }

    #[tokio::test]
    async fn controlled_streaming_restores_privacy_mapping_before_commit() {
        let mut req = request();
        req.stream = Some(true);
        req.messages[0].content = json!("Email alice@example.com");

        let upstream = FakeUpstream::new(response_with_content("Response for [EMAIL_1]"));
        let upstream_state = upstream.state();
        let service = ChatService::new_with_guards(
            Arc::new(FakeExactCache::new()),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(upstream),
            Arc::new(UserMappingGuard),
            ChatServiceSettings {
                semantic_cache_enabled: true,
                exact_cache_enabled: true,
                exact_cache_fail_open: true,
                exact_cache_store_enabled: true,
                semantic_cache_store_enabled: true,
                semantic_cache_fail_open: true,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 500,
            },
            model_prices(),
            None,
        );

        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                false,
            )
            .await
            .unwrap();

        let mut body = streamed.body;
        let mut received = Vec::new();
        while let Some(item) = body.next().await {
            received.extend_from_slice(&item.unwrap());
        }
        let received = String::from_utf8(received).unwrap();

        assert!(received.contains("alice@example.com"));
        assert!(!received.contains("[EMAIL_1]"));
        assert_eq!(upstream_state.lock().unwrap().call_count, 1);
    }

    #[tokio::test]
    async fn controlled_streaming_upstream_failure_exposes_no_partial_response() {
        let mut req = request();
        req.stream = Some(true);

        let sink = RecordingEvidenceSink::default();
        let events = Arc::clone(&sink.events);
        let service = ChatService::new_with_guards_and_evidence(
            ChatServiceDeps {
                exact_cache: Arc::new(FakeExactCache::new()),
                semantic_cache: Arc::new(FakeSemanticCache::new()),
                upstream: Arc::new(FailingStreamUpstream),
                guard_orchestrator: Arc::new(crate::guards::NoopGuardOrchestrator),
                evidence_sink: Arc::new(sink),
                upstream_metadata: UpstreamMetadata {
                    provider_type: "test".into(),
                    provider_name: "test".into(),
                },
                dependencies: DependencyState::new(true, true, true),
                enforcement_mode: AifEnforcementMode::Enforce,
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
        let error = match service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                trace_id,
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                false,
            )
            .await
        {
            Ok(_) => panic!("partial upstream stream must not be committed to the client"),
            Err(error) => error,
        };

        assert_eq!(error.metrics_class(), "upstream_error");

        let events = events.lock().unwrap();
        assert!(events.iter().any(|event| {
            event.trace_id == trace_id && event.event_type == "upstream.stream.failed"
        }));
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    event.trace_id == trace_id && event.event_type == "request.failed"
                })
                .count(),
            1
        );
        assert!(!events.iter().any(|event| {
            event.trace_id == trace_id && event.event_type == "request.completed"
        }));
    }

    #[tokio::test]
    async fn controlled_streaming_idle_timeout_releases_upstream_permit() {
        let mut req = request();
        req.stream = Some(true);

        let service = build_service(
            Arc::new(FakeExactCache::new()),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(StallingStreamUpstream),
            false,
        );

        let error = match service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                false,
            )
            .await
        {
            Ok(_) => panic!("idle provider stream must time out before commit"),
            Err(error) => error,
        };

        assert_eq!(error.metrics_class(), "upstream_timeout");
        assert!(error.message().contains("no body chunk"));
        assert_eq!(service.upstream_limit.available_permits(), 500);
    }

    #[tokio::test]
    async fn controlled_streaming_generation_timeout_stops_drip_feed() {
        let mut req = request();
        req.stream = Some(true);

        let service = build_service(
            Arc::new(FakeExactCache::new()),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(DripFeedStreamUpstream),
            false,
        );

        let error = match service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                false,
            )
            .await
        {
            Ok(_) => panic!("drip-feed provider stream must hit the absolute generation ceiling"),
            Err(error) => error,
        };

        assert_eq!(error.metrics_class(), "upstream_timeout");
        assert!(error.message().contains("maximum generation duration"));
        assert_eq!(service.upstream_limit.available_permits(), 500);
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
    async fn non_string_message_content_skips_semantic_lookup_and_store() {
        let mut req = request();
        req.messages[0].content = json!([
            {"type": "text", "text": "describe this"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}
        ]);

        let exact_cache = FakeExactCache::new();
        let exact_state = exact_cache.state();

        let semantic_cache = FakeSemanticCache::new();
        let semantic_state = semantic_cache.state();

        let upstream = FakeUpstream::new(response_with_usage("array-upstream", 1000, 500));
        let upstream_state = upstream.state();

        let service = build_service(
            Arc::new(exact_cache),
            Arc::new(semantic_cache),
            Arc::new(upstream),
            true,
        );

        let result = service.handle(req).await.unwrap();

        assert_eq!(result.id, "array-upstream");
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

    struct LengthLimitedProgressiveUpstream;

    #[async_trait]
    impl LlmUpstream for LengthLimitedProgressiveUpstream {
        async fn chat_completion(
            &self,
            _req: &ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, AppError> {
            panic!("progressive test must not use buffered completion")
        }
        async fn chat_completion_stream(
            &self,
            _req: &ChatCompletionRequest,
        ) -> Result<UpstreamStreamResponse, AppError> {
            let body = stream::iter(vec![
                Ok::<Bytes, UpstreamStreamError>(Bytes::from_static(
                    b"data: {\"id\":\"limited\",\"model\":\"gpt-4o-mini-2024-07-18\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"partial\"},\"finish_reason\":\"length\"}]}\n\n")),
                Ok(Bytes::from_static(b"data: [DONE]\n\n")),
            ]);
            Ok(UpstreamStreamResponse {
                status: reqwest::StatusCode::OK,
                body: Box::pin(body),
            })
        }
    }

    #[tokio::test]
    async fn progressive_length_limited_completion_is_delivered_but_never_cached() {
        let mut req = request();
        req.stream = Some(true);
        let exact = FakeExactCache::new();
        let exact_state = exact.state();
        let semantic = FakeSemanticCache::new();
        let semantic_state = semantic.state();
        let service = build_service(
            Arc::new(exact),
            Arc::new(semantic),
            Arc::new(LengthLimitedProgressiveUpstream),
            true,
        );
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        let collected: Vec<_> = streamed.body.collect().await;
        assert!(collected.iter().all(Result::is_ok));
        let text = collected
            .into_iter()
            .map(|chunk| String::from_utf8(chunk.unwrap().to_vec()).unwrap())
            .collect::<String>();
        assert!(text.contains("partial"));
        assert!(text.contains("[DONE]"));
        assert_eq!(exact_state.lock().unwrap().set_calls, 0);
        assert_eq!(semantic_state.lock().unwrap().store_calls, 0);
    }

    struct DelayedProgressiveUpstream;

    #[async_trait]
    impl LlmUpstream for DelayedProgressiveUpstream {
        async fn chat_completion(
            &self,
            _req: &ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, AppError> {
            panic!("progressive path must not call buffered completion")
        }

        async fn chat_completion_stream(
            &self,
            _req: &ChatCompletionRequest,
        ) -> Result<UpstreamStreamResponse, AppError> {
            let body = async_stream::stream! {
                yield Ok::<Bytes, UpstreamStreamError>(Bytes::from_static(
                    b"data: {\"id\":\"delayed\",\"model\":\"gpt-4o-mini-2024-07-18\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n"
                ));
                tokio::time::sleep(Duration::from_millis(250)).await;
                yield Ok::<Bytes, UpstreamStreamError>(Bytes::from_static(
                    b"data: {\"id\":\"delayed\",\"model\":\"gpt-4o-mini-2024-07-18\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"},\"finish_reason\":\"stop\"}]}\n\n"
                ));
                yield Ok::<Bytes, UpstreamStreamError>(Bytes::from_static(b"data: [DONE]\n\n"));
            };
            Ok(UpstreamStreamResponse {
                status: reqwest::StatusCode::OK,
                body: Box::pin(body),
            })
        }
    }

    #[tokio::test]
    async fn progressive_streaming_delivers_first_event_before_upstream_finishes() {
        let mut req = request();
        req.stream = Some(true);
        let mut service = build_service(
            Arc::new(FakeExactCache::new()),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(DelayedProgressiveUpstream),
            false,
        );
        service.exact_cache_store_enabled = false;
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        let mut body = streamed.body;
        let first = tokio::time::timeout(Duration::from_millis(150), body.next())
            .await
            .expect("first SSE event should arrive before the delayed second event")
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&first).contains("Hello"));
        let second = tokio::time::timeout(Duration::from_secs(2), body.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&second).contains("world"));
        let done = body.next().await.unwrap().unwrap();
        assert_eq!(done, Bytes::from_static(b"data: [DONE]\n\n"));
        assert!(body.next().await.is_none());
    }

    #[tokio::test]
    async fn progressive_requested_with_cache_storage_delivers_incrementally() {
        let mut req = request();
        req.stream = Some(true);

        let exact = FakeExactCache::new();
        let exact_state = exact.state();

        let upstream = FakeUpstream::new(response_with_content("Stored response"));

        let service = build_service(
            Arc::new(exact),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(upstream),
            false,
        );

        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();

        assert_eq!(exact_state.lock().unwrap().set_calls, 0);

        let items: Vec<_> = streamed.body.collect().await;

        assert!(items.iter().all(Result::is_ok));

        assert!(
            items.len() >= 2,
            "progressive streaming yields SSE events and terminal DONE"
        );

        assert_eq!(exact_state.lock().unwrap().set_calls, 1);
    }

    #[tokio::test]
    async fn progressive_streaming_populates_exact_cache_only_after_completion() {
        let mut req = request();
        req.stream = Some(true);
        let exact = FakeExactCache::new();
        let exact_state = exact.state();
        let service = build_service(
            Arc::new(exact),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(DelayedProgressiveUpstream),
            false,
        );
        let streamed = service
            .handle_stream_with_evidence(
                req.clone(),
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        assert_eq!(exact_state.lock().unwrap().set_calls, 0);
        let mut body = streamed.body;
        let first = tokio::time::timeout(Duration::from_millis(150), body.next())
            .await
            .expect("cache-enabled stream should deliver progressively")
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&first).contains("Hello"));
        assert_eq!(exact_state.lock().unwrap().set_calls, 0);
        let items: Vec<_> = body.collect().await;
        assert!(items.iter().all(Result::is_ok));
        assert_eq!(
            items.last().unwrap().as_ref().unwrap(),
            &Bytes::from_static(b"data: [DONE]\n\n")
        );
        let state = exact_state.lock().unwrap();
        assert_eq!(state.set_calls, 1);
        let mut canonical = req;
        canonical.stream = None;
        let stored: ChatCompletionResponse = serde_json::from_str(
            state
                .entries
                .get(&exact_key_for(&canonical))
                .expect("canonical key stored"),
        )
        .unwrap();
        assert_eq!(stored.choices[0].message.content, json!("Hello world"));
    }

    #[tokio::test]
    async fn progressive_streaming_populates_semantic_cache_after_completion() {
        let mut req = request();
        req.stream = Some(true);
        let semantic = FakeSemanticCache::new();
        let semantic_state = semantic.state();
        let service = build_service(
            Arc::new(FakeExactCache::new()),
            Arc::new(semantic),
            Arc::new(DelayedProgressiveUpstream),
            true,
        );
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        assert_eq!(semantic_state.lock().unwrap().store_calls, 0);
        let items: Vec<_> = streamed.body.collect().await;
        assert!(items.iter().all(Result::is_ok));
        assert_eq!(semantic_state.lock().unwrap().store_calls, 1);
        let state = semantic_state.lock().unwrap();
        assert_eq!(
            state.last_store_response.as_ref().unwrap().choices[0]
                .message
                .content,
            json!("Hello world")
        );
    }

    #[tokio::test]
    async fn progressive_streaming_fail_open_semantic_store_error_keeps_done() {
        let mut req = request();
        req.stream = Some(true);
        let semantic = FakeSemanticCache::with_store_error("simulated write outage");
        let semantic_state = semantic.state();
        let service = build_service(
            Arc::new(FakeExactCache::new()),
            Arc::new(semantic),
            Arc::new(DelayedProgressiveUpstream),
            true,
        );
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        let items: Vec<_> = streamed.body.collect().await;
        assert!(items.iter().all(Result::is_ok));
        assert_eq!(
            items.last().unwrap().as_ref().unwrap(),
            &Bytes::from_static(b"data: [DONE]\n\n")
        );
        assert_eq!(semantic_state.lock().unwrap().store_calls, 1);
    }

    #[tokio::test]
    async fn progressive_streaming_incomplete_upstream_never_populates_cache() {
        let mut req = request();
        req.stream = Some(true);
        let exact = FakeExactCache::new();
        let exact_state = exact.state();
        let service = build_service(
            Arc::new(exact),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(FailingStreamUpstream),
            false,
        );
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        let items: Vec<_> = streamed.body.collect().await;
        assert!(
            items.iter().any(Result::is_err),
            "upstream failure must be visible"
        );
        assert_eq!(exact_state.lock().unwrap().set_calls, 0);
        assert!(!items.iter().any(|event| event
            .as_ref()
            .is_ok_and(|bytes| bytes == &Bytes::from_static(b"data: [DONE]\n\n"))));
    }

    #[tokio::test]
    async fn progressive_streaming_observe_shadow_hit_does_not_overwrite_cache() {
        let mut req = request();
        req.stream = Some(true);
        let mut canonical = req.clone();
        canonical.stream = None;
        let exact = FakeExactCache::with_entry(
            evaluation_exact_key_for(&canonical),
            serde_json::to_string(&response_with_content("shadow-hit")).unwrap(),
        );
        let exact_state = exact.state();
        let service = build_observe_service(
            Arc::new(exact),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(DelayedProgressiveUpstream),
            false,
        );
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        let mut body = streamed.body;
        let first = tokio::time::timeout(Duration::from_millis(150), body.next())
            .await
            .expect("Observe-mode upstream delivery should be progressive")
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&first).contains("Hello"));
        let remaining: Vec<_> = body.collect().await;
        assert!(remaining.iter().all(Result::is_ok));
        assert_eq!(exact_state.lock().unwrap().set_calls, 0);
    }

    #[tokio::test]
    async fn progressive_streaming_strict_exact_cache_store_retains_controlled_path() {
        let mut req = request();
        req.stream = Some(true);
        let mut service = build_service(
            Arc::new(FakeExactCache::new()),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(FakeUpstream::new(response_with_content("strict"))),
            false,
        );
        service.exact_cache_fail_open = false;
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        let items: Vec<_> = streamed.body.collect().await;
        assert_eq!(
            items.len(),
            1,
            "strict cache storage must complete before delivery"
        );
    }

    #[tokio::test]
    async fn progressive_requested_with_exact_cache_hit_replays_immediately() {
        let mut req = request();
        req.stream = Some(true);
        let mut canonical = req.clone();
        canonical.stream = None;
        let key = exact_key_for(&canonical);
        let hit = response_with_content("Cached result");
        let exact = FakeExactCache::with_entry(key, serde_json::to_string(&hit).unwrap());
        let upstream = FakeUpstream::new(response_with_content("Unexpected upstream"));
        let calls = upstream.state();
        let mut service = build_service(
            Arc::new(exact),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(upstream),
            false,
        );
        service.exact_cache_store_enabled = false;
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        let items: Vec<_> = streamed.body.collect().await;
        assert_eq!(items.len(), 1);
        assert!(String::from_utf8_lossy(items[0].as_ref().unwrap()).contains("Cached result"));
        assert_eq!(calls.lock().unwrap().call_count, 0);
    }
    fn progressive_evidence_service(
        upstream: Arc<dyn LlmUpstream>,
    ) -> (ChatService, Arc<Mutex<Vec<EvidenceEvent>>>) {
        let sink = RecordingEvidenceSink::default();
        let events = Arc::clone(&sink.events);
        let service = ChatService::new_with_guards_and_evidence(
            ChatServiceDeps {
                exact_cache: Arc::new(FakeExactCache::new()),
                semantic_cache: Arc::new(FakeSemanticCache::new()),
                upstream,
                guard_orchestrator: Arc::new(crate::guards::NoopGuardOrchestrator),
                evidence_sink: Arc::new(sink),
                upstream_metadata: UpstreamMetadata {
                    provider_type: "test".into(),
                    provider_name: "test".into(),
                },
                dependencies: DependencyState::new(true, true, true),
                enforcement_mode: AifEnforcementMode::Enforce,
            },
            ChatServiceSettings {
                semantic_cache_enabled: false,
                exact_cache_enabled: false,
                exact_cache_fail_open: true,
                exact_cache_store_enabled: false,
                semantic_cache_store_enabled: false,
                semantic_cache_fail_open: true,
                max_prompt_chars: Some(200_000),
                max_inflight_upstream_requests: 1,
            },
            model_prices(),
            None,
        );
        (service, events)
    }

    #[tokio::test]
    async fn progressive_upstream_body_failure_is_not_recorded_as_disconnect() {
        let (service, events) = progressive_evidence_service(Arc::new(FailingStreamUpstream));
        let mut req = request();
        req.stream = Some(true);
        let trace = uuid::Uuid::new_v4();
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                trace,
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        let items: Vec<_> = streamed.body.collect().await;
        assert!(items.iter().any(Result::is_err));
        assert_eq!(service.upstream_limit.available_permits(), 1);
        let events = events.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.trace_id == trace && event.event_type == "request.failed")
                .count(),
            1
        );
        assert!(events
            .iter()
            .any(|event| event.trace_id == trace && event.event_type == "upstream.stream.failed"));
        assert!(!events
            .iter()
            .any(|event| event.trace_id == trace && event.event_type == "request.stream.aborted"));
        assert!(!events
            .iter()
            .any(|event| event.trace_id == trace && event.event_type == "request.completed"));
    }

    #[tokio::test]
    async fn progressive_client_disconnect_releases_permit_and_reports_abort() {
        let (service, events) = progressive_evidence_service(Arc::new(DelayedProgressiveUpstream));
        let mut req = request();
        req.stream = Some(true);
        let trace = uuid::Uuid::new_v4();
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                trace,
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        let mut body = streamed.body;
        assert!(body.next().await.unwrap().is_ok());
        assert_eq!(service.upstream_limit.available_permits(), 0);
        drop(body);
        assert_eq!(service.upstream_limit.available_permits(), 1);
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| event.trace_id == trace && event.event_type == "request.failed")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let events = events.lock().unwrap();
        assert!(events
            .iter()
            .any(|event| event.trace_id == trace && event.event_type == "request.stream.aborted"));
        assert!(!events
            .iter()
            .any(|event| event.trace_id == trace && event.event_type == "request.completed"));
    }

    #[tokio::test]
    async fn progressive_idle_timeout_reports_upstream_failure_and_releases_permit() {
        let (service, events) = progressive_evidence_service(Arc::new(StallingStreamUpstream));
        let mut req = request();
        req.stream = Some(true);
        let trace = uuid::Uuid::new_v4();
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                trace,
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        let items: Vec<_> = streamed.body.collect().await;
        assert!(items.iter().any(Result::is_err));
        assert_eq!(service.upstream_limit.available_permits(), 1);
        let events = events.lock().unwrap();
        assert!(events.iter().any(|event| event.trace_id == trace
            && event.event_type == "upstream.stream.failed"
            && event.attributes.get("error_class") == Some(&json!("stream_idle_timeout"))));
        assert!(!events
            .iter()
            .any(|event| event.trace_id == trace && event.event_type == "request.stream.aborted"));
    }

    #[tokio::test]
    async fn progressive_success_emits_one_completed_event_and_no_abort() {
        let (service, events) = progressive_evidence_service(Arc::new(DelayedProgressiveUpstream));
        let mut req = request();
        req.stream = Some(true);
        let trace = uuid::Uuid::new_v4();
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                trace,
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        let items: Vec<_> = streamed.body.collect().await;
        assert!(items.iter().all(Result::is_ok));
        assert_eq!(service.upstream_limit.available_permits(), 1);
        let events = events.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.trace_id == trace && event.event_type == "request.completed")
                .count(),
            1
        );
        assert!(!events
            .iter()
            .any(|event| event.trace_id == trace && event.event_type == "request.failed"));
    }

    struct BlockedExactCache {
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        completed: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait]
    impl ExactCache for BlockedExactCache {
        async fn get(&self, _key: &str) -> anyhow::Result<Option<String>> {
            Ok(None)
        }

        async fn set(&self, _key: &str, _value: String) -> anyhow::Result<()> {
            self.started.notify_one();
            self.release.notified().await;
            self.completed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn progressive_completed_generation_finishes_cache_write_after_client_disconnect() {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cache = BlockedExactCache {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
            completed: Arc::clone(&completed),
        };
        let service = build_service(
            Arc::new(cache),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(FakeUpstream::new(response_with_content(
                "persist after disconnect",
            ))),
            false,
        );
        let mut req = request();
        req.stream = Some(true);
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        let consumer = tokio::spawn(async move { streamed.body.collect::<Vec<_>>().await });
        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("cache write should start after generation finishes");
        consumer.abort();
        let _ = consumer.await;
        release.notify_one();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !completed.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cache finalization should survive downstream cancellation");
    }

    #[tokio::test]
    async fn progressive_request_with_response_guard_falls_back_to_controlled() {
        let mut req = request();
        req.stream = Some(true);
        req.messages[0].content = json!("Email alice@example.com");
        let upstream = FakeUpstream::new(response_with_content("Response for [EMAIL_1]"));
        let service = ChatService::new_with_guards(
            Arc::new(FakeExactCache::new()),
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
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        let chunks: Vec<_> = streamed.body.collect().await;
        assert_eq!(
            chunks.len(),
            1,
            "fallback must deliver one complete SSE body"
        );
        let bytes = chunks[0].as_ref().unwrap();
        let wire = String::from_utf8_lossy(bytes);
        assert!(wire.contains("alice@example.com"));
        assert!(!wire.contains("[EMAIL_1]"));
        assert!(wire.contains("data: [DONE]"));
    }

    #[tokio::test]
    async fn progressive_cache_hit_is_immediate_complete_sse_without_upstream() {
        let mut req = request();
        req.stream = Some(true);
        let mut canonical = req.clone();
        canonical.stream = None;
        let cached = response_with_content("Immediate cached answer");
        let upstream = FakeUpstream::new(response_with_content("must not be used"));
        let upstream_state = upstream.state();
        let service = build_service(
            Arc::new(FakeExactCache::with_entry(
                exact_key_for(&canonical),
                serde_json::to_string(&cached).unwrap(),
            )),
            Arc::new(FakeSemanticCache::new()),
            Arc::new(upstream),
            false,
        );
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(1), streamed.body.collect::<Vec<_>>())
            .await
            .expect("cached SSE must be available without upstream generation");
        assert_eq!(first.len(), 1);
        let wire = String::from_utf8_lossy(first[0].as_ref().unwrap());
        assert!(wire.contains("Immediate cached answer"));
        assert!(wire.contains("data: [DONE]"));
        assert_eq!(upstream_state.lock().unwrap().call_count, 0);
    }

    #[tokio::test]
    async fn progressive_semantic_hit_is_immediate_complete_sse_without_upstream() {
        let mut req = request();
        req.stream = Some(true);
        let upstream = FakeUpstream::new(response_with_content("must not be used"));
        let upstream_state = upstream.state();
        let service = build_service(
            Arc::new(FakeExactCache::new()),
            Arc::new(FakeSemanticCache::with_lookup_result(SemanticLookupHit {
                response: response_with_content("Semantic cached answer"),
                embedding_usage: None,
            })),
            Arc::new(upstream),
            true,
        );
        let streamed = service
            .handle_stream_with_evidence(
                req,
                CacheControl::default(),
                uuid::Uuid::new_v4(),
                crate::streaming::DEFAULT_MAX_CONTROLLED_STREAM_BYTES,
                true,
            )
            .await
            .unwrap();
        let chunks: Vec<_> = streamed.body.collect().await;
        assert_eq!(chunks.len(), 1);
        let wire = String::from_utf8_lossy(chunks[0].as_ref().unwrap());
        assert!(wire.contains("Semantic cached answer"));
        assert!(wire.contains("data: [DONE]"));
        assert_eq!(upstream_state.lock().unwrap().call_count, 0);
    }
}
