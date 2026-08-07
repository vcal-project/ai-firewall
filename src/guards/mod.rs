use crate::metrics;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

use crate::{
    config::{Config, PrivacyGuardMode},
    error::AppError,
    evidence::{
        DataFinding, DecisionEvidence, EventCategory, EventOutcome, EvidenceEvent, EvidenceSink,
        EvidenceSource,
    },
    services::chat_service::CacheControl,
    types::openai::{ChatCompletionRequest, ChatCompletionResponse},
};

mod privacy;
mod security_guard;

pub use privacy::PrivacyGuardOrchestrator;
pub use security_guard::SecurityGuardClient;

pub struct CompositeGuardOrchestrator {
    security: Option<SecurityGuardClient>,
    privacy: Option<PrivacyGuardOrchestrator>,
    evidence_sink: Arc<dyn EvidenceSink>,
    guard_fail_open: bool,
}

#[async_trait]
impl GuardOrchestrator for CompositeGuardOrchestrator {
    async fn before_cache(
        &self,
        request: ChatCompletionRequest,
        trace_id: Uuid,
    ) -> Result<GuardedRequest, AppError> {
        let mut guarded = GuardedRequest {
            request,
            context: GuardContext::default(),
            cache_control: CacheControl::default(),
        };

        if let Some(security) = &self.security {
            let started = Instant::now();

            match security.scan_request(&guarded.request).await {
                Ok(()) => {
                    self.emit_security_event(
                        trace_id,
                        "request",
                        EventOutcome::Allowed,
                        None,
                        started.elapsed(),
                    )
                    .await;
                    metrics::observe_guard_request("security", "request", "allow");
                    metrics::observe_guard_latency_seconds(
                        "security",
                        "request",
                        started.elapsed().as_secs_f64(),
                    );
                }
                Err(err) => {
                    let result = if err.is_security_block() {
                        "block"
                    } else {
                        "error"
                    };

                    metrics::observe_guard_request("security", "request", result);
                    metrics::observe_guard_latency_seconds(
                        "security",
                        "request",
                        started.elapsed().as_secs_f64(),
                    );

                    self.emit_security_event(
                        trace_id,
                        "request",
                        if err.is_security_block() {
                            EventOutcome::Blocked
                        } else {
                            EventOutcome::Failed
                        },
                        Some(&err),
                        started.elapsed(),
                    )
                    .await;

                    if err.is_security_block() {
                        metrics::observe_security_block(
                            err.security_block_stage().unwrap_or("request"),
                            err.security_block_rule_id(),
                        );
                        return Err(err);
                    }

                    if self.guard_fail_open {
                        tracing::warn!(
                            guard = "security",
                            stage = "request",
                            error = %err.message(),
                            "Security Guard request scan failed; guard_fail_open=true so request continues"
                        );
                    } else {
                        return Err(err);
                    }
                }
            }
        }

        if let Some(privacy) = &self.privacy {
            let started = Instant::now();

            match privacy.before_cache(guarded.request, trace_id).await {
                Ok(next_guarded) => {
                    self.emit_privacy_scan_event(
                        trace_id,
                        &next_guarded.context,
                        started.elapsed(),
                    )
                    .await;
                    metrics::observe_guard_request(
                        "privacy",
                        "request",
                        next_guarded.context.privacy_metric_result(),
                    );
                    metrics::observe_guard_latency_seconds(
                        "privacy",
                        "request",
                        started.elapsed().as_secs_f64(),
                    );
                    guarded = next_guarded;
                }
                Err(err) => {
                    self.emit_privacy_failure_event(
                        trace_id,
                        "request",
                        "guard.privacy.request.failed",
                        &err,
                        started.elapsed(),
                    )
                    .await;
                    metrics::observe_guard_request("privacy", "request", "error");
                    metrics::observe_guard_latency_seconds(
                        "privacy",
                        "request",
                        started.elapsed().as_secs_f64(),
                    );
                    return Err(err);
                }
            }
        }

        Ok(guarded)
    }

    async fn before_response_restore(
        &self,
        _context: &GuardContext,
        response: ChatCompletionResponse,
        trace_id: Uuid,
    ) -> Result<ChatCompletionResponse, AppError> {
        if let Some(security) = &self.security {
            let started = Instant::now();

            match security.scan_response(&response).await {
                Ok(()) => {
                    self.emit_security_event(
                        trace_id,
                        "response",
                        EventOutcome::Allowed,
                        None,
                        started.elapsed(),
                    )
                    .await;
                    metrics::observe_guard_request("security", "response", "allow");
                    metrics::observe_guard_latency_seconds(
                        "security",
                        "response",
                        started.elapsed().as_secs_f64(),
                    );
                }
                Err(err) => {
                    let result = if err.is_security_block() {
                        "block"
                    } else {
                        "error"
                    };

                    metrics::observe_guard_request("security", "response", result);
                    metrics::observe_guard_latency_seconds(
                        "security",
                        "response",
                        started.elapsed().as_secs_f64(),
                    );

                    self.emit_security_event(
                        trace_id,
                        "response",
                        if err.is_security_block() {
                            EventOutcome::Blocked
                        } else {
                            EventOutcome::Failed
                        },
                        Some(&err),
                        started.elapsed(),
                    )
                    .await;

                    if err.is_security_block() {
                        metrics::observe_security_block(
                            err.security_block_stage().unwrap_or("response"),
                            err.security_block_rule_id(),
                        );

                        if self.privacy.is_some() {
                            metrics::observe_privacy_restore_skipped("security_response_blocked");
                        }
                        return Err(err);
                    }

                    if self.guard_fail_open {
                        tracing::warn!(
                            guard = "security",
                            stage = "response",
                            error = %err.message(),
                            "Security Guard response scan failed; guard_fail_open=true so response continues"
                        );
                    } else {
                        return Err(err);
                    }
                }
            }
        }

        Ok(response)
    }

    async fn restore_response(
        &self,
        context: &GuardContext,
        response: ChatCompletionResponse,
        trace_id: Uuid,
    ) -> Result<ChatCompletionResponse, AppError> {
        if let Some(privacy) = &self.privacy {
            let started = Instant::now();

            match privacy.restore_response(context, response, trace_id).await {
                Ok(restored) => {
                    let outcome = if context.privacy_mapping_id.is_some() {
                        EventOutcome::Completed
                    } else {
                        EventOutcome::Skipped
                    };
                    self.emit_privacy_restore_event(
                        trace_id,
                        context,
                        outcome,
                        None,
                        started.elapsed(),
                    )
                    .await;
                    metrics::observe_guard_request("privacy", "response", "restored");
                    metrics::observe_guard_latency_seconds(
                        "privacy",
                        "response",
                        started.elapsed().as_secs_f64(),
                    );
                    Ok(restored)
                }
                Err(err) => {
                    self.emit_privacy_restore_event(
                        trace_id,
                        context,
                        EventOutcome::Failed,
                        Some(&err),
                        started.elapsed(),
                    )
                    .await;
                    metrics::observe_guard_request("privacy", "response", "error");
                    metrics::observe_guard_latency_seconds(
                        "privacy",
                        "response",
                        started.elapsed().as_secs_f64(),
                    );
                    Err(err)
                }
            }
        } else {
            Ok(response)
        }
    }
}

impl CompositeGuardOrchestrator {
    async fn emit(&self, event: EvidenceEvent) {
        if let Err(error) = self.evidence_sink.emit(event).await {
            tracing::warn!(error = %error, "failed to emit guard evidence event");
        }
    }

    async fn emit_security_event(
        &self,
        trace_id: Uuid,
        stage: &'static str,
        outcome: EventOutcome,
        error: Option<&AppError>,
        elapsed: std::time::Duration,
    ) {
        let mut event = EvidenceEvent::new(
            trace_id,
            EvidenceSource::SecurityGuard,
            EventCategory::Security,
            format!("guard.security.{stage}.scan"),
            outcome,
        );
        event
            .attributes
            .insert("stage".into(), Value::String(stage.into()));
        event
            .attributes
            .insert("latency_ms".into(), Value::from(elapsed.as_millis() as u64));
        if let Some(error) = error {
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
                "error_class".into(),
                Value::String(error.metrics_class().into()),
            );
        } else {
            event.decision = Some(DecisionEvidence {
                action: "allow".into(),
                reason_code: "security_scan_allowed".into(),
                rule_id: None,
                severity: None,
            });
        }
        self.emit(event).await;
    }

    async fn emit_privacy_scan_event(
        &self,
        trace_id: Uuid,
        context: &GuardContext,
        elapsed: std::time::Duration,
    ) {
        let outcome = if context.privacy_failure_reason.is_some() {
            EventOutcome::Failed
        } else if context.privacy_scan_skipped {
            EventOutcome::Skipped
        } else {
            EventOutcome::Allowed
        };
        let mut event = EvidenceEvent::new(
            trace_id,
            EvidenceSource::PrivacyGuard,
            EventCategory::Privacy,
            "guard.privacy.request.scan",
            outcome,
        );
        event.findings = context.privacy_findings.clone();
        event
            .attributes
            .insert("stage".into(), Value::String("request".into()));
        event.attributes.insert(
            "mode".into(),
            Value::String(
                context
                    .privacy_mode
                    .clone()
                    .unwrap_or_else(|| "unknown".into()),
            ),
        );
        event.attributes.insert(
            "action".into(),
            Value::String(
                context
                    .privacy_action
                    .clone()
                    .unwrap_or_else(|| "none".into()),
            ),
        );
        event
            .attributes
            .insert("modified".into(), Value::Bool(context.privacy_modified));
        event.attributes.insert(
            "mapping_created".into(),
            Value::Bool(context.privacy_mapping_id.is_some()),
        );
        event
            .attributes
            .insert("latency_ms".into(), Value::from(elapsed.as_millis() as u64));
        if let Some(reason) = &context.privacy_failure_reason {
            event.decision = Some(DecisionEvidence {
                action: "continue_fail_open".into(),
                reason_code: "privacy_guard_fail_open".into(),
                rule_id: None,
                severity: None,
            });
            event
                .attributes
                .insert("failure_reason".into(), Value::String(reason.clone()));
        }
        self.emit(event).await;
    }

    async fn emit_privacy_failure_event(
        &self,
        trace_id: Uuid,
        stage: &'static str,
        event_type: &'static str,
        error: &AppError,
        elapsed: std::time::Duration,
    ) {
        let mut event = EvidenceEvent::new(
            trace_id,
            EvidenceSource::PrivacyGuard,
            EventCategory::Privacy,
            event_type,
            EventOutcome::Failed,
        );
        event
            .attributes
            .insert("stage".into(), Value::String(stage.into()));
        event
            .attributes
            .insert("latency_ms".into(), Value::from(elapsed.as_millis() as u64));
        event.attributes.insert(
            "error_class".into(),
            Value::String(error.metrics_class().into()),
        );
        event.decision = Some(DecisionEvidence {
            action: "fail_request".into(),
            reason_code: error.evidence_reason_code().into(),
            rule_id: None,
            severity: None,
        });
        self.emit(event).await;
    }

    async fn emit_privacy_restore_event(
        &self,
        trace_id: Uuid,
        context: &GuardContext,
        outcome: EventOutcome,
        error: Option<&AppError>,
        elapsed: std::time::Duration,
    ) {
        let mut event = EvidenceEvent::new(
            trace_id,
            EvidenceSource::PrivacyGuard,
            EventCategory::Privacy,
            "guard.privacy.response.restore",
            outcome,
        );
        event
            .attributes
            .insert("stage".into(), Value::String("response".into()));
        event.attributes.insert(
            "mapping_present".into(),
            Value::Bool(context.privacy_mapping_id.is_some()),
        );
        event
            .attributes
            .insert("latency_ms".into(), Value::from(elapsed.as_millis() as u64));
        if let Some(error) = error {
            event.attributes.insert(
                "error_class".into(),
                Value::String(error.metrics_class().into()),
            );
            event.decision = Some(DecisionEvidence {
                action: "fail_request".into(),
                reason_code: error.evidence_reason_code().into(),
                rule_id: None,
                severity: None,
            });
        }
        self.emit(event).await;
    }
}

#[derive(Clone, Debug, Default)]
pub struct GuardContext {
    pub privacy_mapping_id: Option<String>,
    pub privacy_tenant_id: Option<String>,
    /// Deterministic placeholder/entity signature for semantic cache isolation.
    /// Example: EMAIL:1|IP:1|PHONE:0|JWT:0|API_KEY:0|BEARER_TOKEN:0|PRIVATE_KEY:0|CREDIT_CARD_LIKE:0|OTHER:0
    pub privacy_placeholder_signature: Option<String>,
    pub privacy_findings: Vec<DataFinding>,
    pub privacy_action: Option<String>,
    pub privacy_mode: Option<String>,
    pub privacy_modified: bool,
    pub privacy_scan_skipped: bool,
    pub privacy_failure_reason: Option<String>,
}

impl GuardContext {
    fn privacy_metric_result(&self) -> &'static str {
        if self.privacy_failure_reason.is_some() {
            "error"
        } else if self.privacy_scan_skipped {
            "skipped"
        } else if self.privacy_modified {
            "anonymized"
        } else {
            "allow"
        }
    }
}

#[derive(Clone, Debug)]
pub struct GuardedRequest {
    pub request: ChatCompletionRequest,
    pub context: GuardContext,
    pub cache_control: CacheControl,
}

#[async_trait]
pub trait GuardOrchestrator: Send + Sync {
    async fn before_cache(
        &self,
        request: ChatCompletionRequest,
        trace_id: Uuid,
    ) -> Result<GuardedRequest, AppError>;

    async fn before_response_restore(
        &self,
        _context: &GuardContext,
        response: ChatCompletionResponse,
        _trace_id: Uuid,
    ) -> Result<ChatCompletionResponse, AppError> {
        Ok(response)
    }

    async fn restore_response(
        &self,
        context: &GuardContext,
        response: ChatCompletionResponse,
        trace_id: Uuid,
    ) -> Result<ChatCompletionResponse, AppError>;
}

pub struct NoopGuardOrchestrator;

#[async_trait]
impl GuardOrchestrator for NoopGuardOrchestrator {
    async fn before_cache(
        &self,
        request: ChatCompletionRequest,
        _trace_id: Uuid,
    ) -> Result<GuardedRequest, AppError> {
        Ok(GuardedRequest {
            request,
            context: GuardContext::default(),
            cache_control: CacheControl::default(),
        })
    }

    async fn restore_response(
        &self,
        _context: &GuardContext,
        response: ChatCompletionResponse,
        _trace_id: Uuid,
    ) -> Result<ChatCompletionResponse, AppError> {
        Ok(response)
    }
}

pub fn build_guard_orchestrator(
    cfg: &Config,
    evidence_sink: Arc<dyn EvidenceSink>,
) -> anyhow::Result<Arc<dyn GuardOrchestrator>> {
    let security = if cfg.security_guard_enabled {
        Some(
            SecurityGuardClient::new(
                cfg.security_guard_url.clone(),
                cfg.security_guard_api_key.clone(),
                cfg.security_guard_timeout_seconds,
            )
            .map_err(|e| {
                anyhow::anyhow!(
                    "failed to initialize Security Guard client: {}",
                    e.message()
                )
            })?,
        )
    } else {
        None
    };

    let privacy = if cfg.privacy_guard_enabled {
        Some(PrivacyGuardOrchestrator::new(
            cfg.privacy_guard_url.clone(),
            cfg.privacy_guard_api_key.clone(),
            cfg.privacy_guard_mode,
            cfg.privacy_guard_restore_enabled,
            cfg.privacy_guard_tenant_id.clone(),
            cfg.privacy_guard_policy_id.clone(),
            cfg.guard_fail_open,
            std::time::Duration::from_secs(cfg.privacy_guard_timeout_seconds),
        ))
    } else {
        None
    };

    if security.is_none() && privacy.is_none() {
        Ok(Arc::new(NoopGuardOrchestrator))
    } else {
        Ok(Arc::new(CompositeGuardOrchestrator {
            security,
            privacy,
            evidence_sink,
            guard_fail_open: cfg.guard_fail_open,
        }))
    }
}

pub fn privacy_mode_as_str(mode: PrivacyGuardMode) -> &'static str {
    match mode {
        PrivacyGuardMode::DetectOnly => "detect_only",
        PrivacyGuardMode::Redact => "redact",
        PrivacyGuardMode::Anonymize => "anonymize",
        PrivacyGuardMode::Block => "block",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, EmbeddingPrice, ModelPrice, PrivacyGuardMode, ProviderKind};
    use std::collections::HashMap;

    fn test_config(security_enabled: bool, privacy_enabled: bool) -> Config {
        let mut model_prices = HashMap::new();
        model_prices.insert(
            "gpt-4o-mini".to_string(),
            ModelPrice {
                input_usd_per_1m_tokens: 0.150,
                output_usd_per_1m_tokens: 0.600,
            },
        );

        Config {
            config_version: crate::release::CONFIG_SCHEMA_VERSION,
            listen_addr: "127.0.0.1:8080".to_string(),
            redis_url: "redis://127.0.0.1:6379".to_string(),
            redis_timeout_seconds: 2,

            upstream_provider: ProviderKind::OpenAiCompatible,
            upstream_base_url: "http://127.0.0.1:9000".to_string(),
            upstream_api_key: "test-upstream-key".to_string(),

            embedding_provider: ProviderKind::OpenAiCompatible,
            embedding_base_url: "http://127.0.0.1:9000".to_string(),
            embedding_api_key: "test-embedding-key".to_string(),
            embedding_model: "text-embedding-3-small".to_string(),
            embedding_price: Some(EmbeddingPrice {
                usd_per_1m_tokens: 0.020,
            }),

            qdrant_url: "http://127.0.0.1:6334".to_string(),
            qdrant_api_key: None,
            qdrant_collection: "ai_firewall_test".to_string(),
            qdrant_vector_size: 1536,

            cache_ttl_seconds: 86_400,
            exact_cache_ttl_seconds: 86_400,
            semantic_cache_retention_seconds: 86_400,
            request_timeout_seconds: 30,
            upstream_timeout_seconds: 30,
            embedding_timeout_seconds: 30,
            graceful_shutdown_timeout_seconds: 10,
            max_request_body_bytes: 1_048_576,
            max_prompt_chars: 200_000,
            max_inflight_requests: 1000,
            max_inflight_upstream_requests: 500,

            exact_cache_enabled: true,
            exact_cache_fail_open: true,
            exact_cache_store_enabled: true,

            semantic_cache_enabled: true,
            semantic_similarity_threshold: 0.92,
            semantic_cache_fail_open: true,
            semantic_cache_store_enabled: true,

            security_guard_enabled: security_enabled,
            security_guard_url: "http://127.0.0.1:8091".to_string(),
            security_guard_api_key: Some("test-security-key".to_string()),
            security_guard_timeout_seconds: 1,

            privacy_guard_enabled: privacy_enabled,
            privacy_guard_url: "http://127.0.0.1:8090".to_string(),
            privacy_guard_api_key: Some("test-privacy-key".to_string()),
            privacy_guard_mode: PrivacyGuardMode::Anonymize,
            privacy_guard_restore_enabled: true,
            privacy_guard_tenant_id: None,
            privacy_guard_policy_id: None,
            privacy_guard_timeout_seconds: 1,
            guard_fail_open: false,

            audit_enabled: false,
            audit_url: "http://127.0.0.1:8092".to_string(),
            audit_api_key: None,
            audit_producer_instance_id: "test-instance".to_string(),
            audit_queue_capacity: 100,
            audit_batch_size: 10,
            audit_flush_interval_ms: 1_000,
            audit_timeout_seconds: 5,
            audit_retry_max_attempts: 3,
            audit_retry_initial_backoff_ms: 100,
            audit_retry_max_backoff_ms: 5_000,

            cache_bypass_header: "X-AIF-Cache-Bypass".to_string(),
            metrics_auth_required: false,
            metrics_auth_token: None,

            readiness_requires_redis: false,
            readiness_requires_qdrant: false,
            readiness_requires_upstream: false,

            model_prices,

            allow_unknown_models_pass_through: true,
        }
    }

    fn test_request() -> crate::types::openai::ChatCompletionRequest {
        crate::types::openai::ChatCompletionRequest {
            model: "gpt-4o-mini".to_string(),
            messages: vec![crate::types::openai::ChatMessage {
                role: "user".to_string(),
                content: serde_json::json!("failure injection request"),
                name: None,
                extra: serde_json::Map::new(),
            }],
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: None,
            extra: serde_json::Map::new(),
        }
    }

    #[tokio::test]
    async fn security_transport_failure_respects_fail_open() {
        let mut cfg = test_config(true, false);
        cfg.security_guard_url = "http://127.0.0.1:9".to_string();
        cfg.guard_fail_open = true;
        let guard = build_guard_orchestrator(&cfg, Arc::new(crate::evidence::NoopEvidenceSink))
            .expect("guard should initialize");

        let result = guard
            .before_cache(test_request(), uuid::Uuid::new_v4())
            .await;

        assert!(
            result.is_ok(),
            "Security Guard outage must fail open when configured"
        );
    }

    #[tokio::test]
    async fn security_transport_failure_respects_fail_closed() {
        let mut cfg = test_config(true, false);
        cfg.security_guard_url = "http://127.0.0.1:9".to_string();
        cfg.guard_fail_open = false;
        let guard = build_guard_orchestrator(&cfg, Arc::new(crate::evidence::NoopEvidenceSink))
            .expect("guard should initialize");

        let error = guard
            .before_cache(test_request(), uuid::Uuid::new_v4())
            .await
            .expect_err("Security Guard outage must fail closed when configured");

        assert_eq!(error.metrics_class(), "security_guard_unavailable");
    }

    #[test]
    fn all_four_guard_module_combinations_build_successfully() {
        let combinations = [
            (false, false, "core_only"),
            (false, true, "privacy_only"),
            (true, false, "security_only"),
            (true, true, "security_and_privacy"),
        ];

        for (security_enabled, privacy_enabled, mode) in combinations {
            let cfg = test_config(security_enabled, privacy_enabled);
            let _orchestrator =
                build_guard_orchestrator(&cfg, Arc::new(crate::evidence::NoopEvidenceSink))
                    .expect("guard orchestrator should initialize");

            tracing::debug!(mode, "guard orchestrator built successfully");
        }
    }
}
