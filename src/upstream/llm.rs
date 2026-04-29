use async_trait::async_trait;

use crate::error::AppError;
use crate::types::openai::{ChatCompletionRequest, ChatCompletionResponse};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamErrorKind {
    Timeout,
    Tls,
    Dns,
    Connect,
    HttpStatus,
    Other,
}

impl UpstreamErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            UpstreamErrorKind::Timeout => "upstream_timeout",
            UpstreamErrorKind::Tls => "upstream_tls_error",
            UpstreamErrorKind::Dns => "upstream_dns_error",
            UpstreamErrorKind::Connect => "upstream_connect_error",
            UpstreamErrorKind::HttpStatus => "upstream_http_error",
            UpstreamErrorKind::Other => "upstream_error",
        }
    }

    pub fn default_message(self) -> &'static str {
        match self {
            UpstreamErrorKind::Timeout => {
                "The upstream provider did not respond before the configured timeout."
            }
            UpstreamErrorKind::Tls => "Upstream TLS certificate verification failed.",
            UpstreamErrorKind::Dns => "Failed to resolve upstream provider hostname.",
            UpstreamErrorKind::Connect => "Failed to connect to upstream provider.",
            UpstreamErrorKind::HttpStatus => "The upstream provider returned an error response.",
            UpstreamErrorKind::Other => "The upstream provider request failed.",
        }
    }

    pub fn default_hint(self) -> Option<&'static str> {
        match self {
            UpstreamErrorKind::Tls => Some(
                "Check whether the upstream certificate is trusted by the AI Firewall container and whether upstream_base_url matches the certificate Subject Alternative Name.",
            ),
            UpstreamErrorKind::Dns => Some(
                "Check upstream_base_url and DNS/network configuration from inside the AI Firewall container.",
            ),
            UpstreamErrorKind::Connect => Some(
                "Check that the upstream endpoint is reachable from the AI Firewall container and that the port is open.",
            ),
            UpstreamErrorKind::Timeout => Some(
                "Increase request_timeout_seconds or check upstream provider latency and availability.",
            ),
            UpstreamErrorKind::HttpStatus | UpstreamErrorKind::Other => None,
        }
    }

    pub fn metrics_class(self) -> &'static str {
        self.as_str()
    }
}

#[async_trait]
pub trait LlmUpstream: Send + Sync {
    async fn chat_completion(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, AppError>;
}
