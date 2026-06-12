use async_trait::async_trait;
use std::sync::Arc;

use crate::{
    config::{Config, PrivacyGuardMode},
    error::AppError,
    services::chat_service::CacheControl,
    types::openai::{ChatCompletionRequest, ChatCompletionResponse},
};

mod privacy;

pub use privacy::PrivacyGuardOrchestrator;

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
    /// Return true when this guard cannot safely process streaming/SSE responses.
    ///
    /// AI Cost Firewall currently handles Privacy Guard restoration on complete
    /// non-streaming responses. Until SSE restoration is implemented, privacy
    /// guarded requests must not be allowed to bypass the guard through
    /// `stream=true`.
    fn reject_streaming_requests(&self) -> bool {
        false
    }

    async fn before_cache(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<GuardedRequest, AppError>;

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
    if cfg.privacy_guard_enabled {
        Arc::new(PrivacyGuardOrchestrator::new(
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
        Arc::new(NoopGuardOrchestrator)
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
