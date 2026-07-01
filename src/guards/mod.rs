use crate::metrics;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;

use crate::{
    config::{Config, PrivacyGuardMode},
    error::AppError,
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
}

#[async_trait]
impl GuardOrchestrator for CompositeGuardOrchestrator {
    fn reject_streaming_requests(&self) -> bool {
        self.security.is_some() || self.privacy.is_some()
    }

    async fn before_cache(
        &self,
        request: ChatCompletionRequest,
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
                    metrics::observe_guard_request("security", "request", "allow");
                    metrics::observe_guard_latency_seconds(
                        "security",
                        "request",
                        started.elapsed().as_secs_f64(),
                    );
                }
                Err(err) => {
                    metrics::observe_guard_request("security", "request", "error");
                    metrics::observe_guard_latency_seconds(
                        "security",
                        "request",
                        started.elapsed().as_secs_f64(),
                    );

                    if err.metrics_class() == "security_request_blocked" {
                        metrics::observe_security_block("request", None);
                    }

                    return Err(err);
                }
            }
        }

        if let Some(privacy) = &self.privacy {
            let started = Instant::now();

            match privacy.before_cache(guarded.request).await {
                Ok(next_guarded) => {
                    metrics::observe_guard_request("privacy", "request", "anonymized");
                    metrics::observe_guard_latency_seconds(
                        "privacy",
                        "request",
                        started.elapsed().as_secs_f64(),
                    );
                    guarded = next_guarded;
                }
                Err(err) => {
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
    ) -> Result<ChatCompletionResponse, AppError> {
        if let Some(security) = &self.security {
            let started = Instant::now();

            match security.scan_response(&response).await {
                Ok(()) => {
                    metrics::observe_guard_request("security", "response", "allow");
                    metrics::observe_guard_latency_seconds(
                        "security",
                        "response",
                        started.elapsed().as_secs_f64(),
                    );
                }
                Err(err) => {
                    metrics::observe_guard_request("security", "response", "error");
                    metrics::observe_guard_latency_seconds(
                        "security",
                        "response",
                        started.elapsed().as_secs_f64(),
                    );

                    if err.metrics_class() == "security_response_blocked" {
                        metrics::observe_security_block("response", None);

                        if self.privacy.is_some() {
                            metrics::observe_privacy_restore_skipped("security_response_blocked");
                        }
                    }

                    return Err(err);
                }
            }
        }

        Ok(response)
    }

    async fn restore_response(
        &self,
        context: &GuardContext,
        response: ChatCompletionResponse,
    ) -> Result<ChatCompletionResponse, AppError> {
        if let Some(privacy) = &self.privacy {
            let started = Instant::now();

            match privacy.restore_response(context, response).await {
                Ok(restored) => {
                    metrics::observe_guard_request("privacy", "response", "restored");
                    metrics::observe_guard_latency_seconds(
                        "privacy",
                        "response",
                        started.elapsed().as_secs_f64(),
                    );
                    Ok(restored)
                }
                Err(err) => {
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

#[derive(Clone, Debug, Default)]
pub struct GuardContext {
    pub privacy_mapping_id: Option<String>,
    pub privacy_tenant_id: Option<String>,
    /// Deterministic placeholder/entity signature for semantic cache isolation.
    /// Example: EMAIL:1|IP:1|PHONE:0|JWT:0|API_KEY:0|BEARER_TOKEN:0|PRIVATE_KEY:0|CREDIT_CARD_LIKE:0|OTHER:0
    pub privacy_placeholder_signature: Option<String>,
}

#[derive(Clone, Debug)]
pub struct GuardedRequest {
    pub request: ChatCompletionRequest,
    pub context: GuardContext,
    pub cache_control: CacheControl,
}

#[async_trait]
pub trait GuardOrchestrator: Send + Sync {
    fn reject_streaming_requests(&self) -> bool {
        false
    }

    async fn before_cache(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<GuardedRequest, AppError>;

    async fn before_response_restore(
        &self,
        _context: &GuardContext,
        response: ChatCompletionResponse,
    ) -> Result<ChatCompletionResponse, AppError> {
        Ok(response)
    }

    async fn restore_response(
        &self,
        context: &GuardContext,
        response: ChatCompletionResponse,
    ) -> Result<ChatCompletionResponse, AppError>;
}

pub struct NoopGuardOrchestrator;

#[async_trait]
impl GuardOrchestrator for NoopGuardOrchestrator {
    async fn before_cache(
        &self,
        request: ChatCompletionRequest,
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
    ) -> Result<ChatCompletionResponse, AppError> {
        Ok(response)
    }
}

pub fn build_guard_orchestrator(cfg: &Config) -> Arc<dyn GuardOrchestrator> {
    let security = if cfg.security_guard_enabled {
        Some(
            SecurityGuardClient::new(
                cfg.security_guard_url.clone(),
                cfg.security_guard_api_key.clone(),
                cfg.security_guard_timeout_seconds,
            )
            .unwrap_or_else(|e| {
                tracing::error!(
                    error = %e.message(),
                    "failed to initialize Security Guard client"
                );
                panic!("failed to initialize Security Guard client");
            }),
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
        Arc::new(NoopGuardOrchestrator)
    } else {
        Arc::new(CompositeGuardOrchestrator { security, privacy })
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
            listen_addr: "127.0.0.1:8080".to_string(),
            redis_url: "redis://127.0.0.1:6379".to_string(),

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

    #[test]
    fn builds_core_only_orchestrator() {
        let cfg = test_config(false, false);

        let orchestrator = build_guard_orchestrator(&cfg);

        assert!(
            !orchestrator.reject_streaming_requests(),
            "core-only AI Firewall mode should not reject streaming because no guard module is enabled"
        );
    }

    #[test]
    fn builds_privacy_only_orchestrator() {
        let cfg = test_config(false, true);

        let orchestrator = build_guard_orchestrator(&cfg);

        assert!(
            orchestrator.reject_streaming_requests(),
            "Privacy Guard mode should reject streaming until guarded streaming is implemented"
        );
    }

    #[test]
    fn builds_security_only_orchestrator() {
        let cfg = test_config(true, false);

        let orchestrator = build_guard_orchestrator(&cfg);

        assert!(
            orchestrator.reject_streaming_requests(),
            "Security Guard mode should reject streaming until guarded streaming is implemented"
        );
    }

    #[test]
    fn builds_security_and_privacy_orchestrator() {
        let cfg = test_config(true, true);

        let orchestrator = build_guard_orchestrator(&cfg);

        assert!(
            orchestrator.reject_streaming_requests(),
            "Security + Privacy Guard mode should reject streaming until guarded streaming is implemented"
        );
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
            let orchestrator = build_guard_orchestrator(&cfg);

            let expected_stream_rejection = security_enabled || privacy_enabled;

            assert_eq!(
                orchestrator.reject_streaming_requests(),
                expected_stream_rejection,
                "unexpected streaming rejection behavior for mode {mode}"
            );
        }
    }
}
