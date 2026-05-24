use async_trait::async_trait;

use crate::error::AppError;
use crate::types::openai::{ChatCompletionRequest, ChatCompletionResponse};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamErrorKind {
    Timeout,
    Tls,
    Dns,
    Connect,
    Authentication,
    NotFound,
    RateLimited,
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
            UpstreamErrorKind::Authentication => "upstream_authentication_error",
            UpstreamErrorKind::NotFound => "upstream_not_found",
            UpstreamErrorKind::RateLimited => "upstream_rate_limited",
        }
    }

    pub fn default_message(self) -> &'static str {
        match self {
            UpstreamErrorKind::Timeout => {
                "The upstream provider did not respond before the configured timeout."
            }
            UpstreamErrorKind::Tls => "TLS/certificate verification failed while contacting the upstream provider.",
            UpstreamErrorKind::Dns => "Failed to resolve the upstream provider hostname.",
            UpstreamErrorKind::Connect => "Failed to connect to the upstream provider host or port.",
            UpstreamErrorKind::HttpStatus => "The upstream provider returned an HTTP error response.",
            UpstreamErrorKind::Other => "The upstream provider request failed before a valid response was received.",
            UpstreamErrorKind::Authentication => "The upstream provider rejected authentication.",
            UpstreamErrorKind::NotFound => "The upstream provider returned 404 for the OpenAI-compatible endpoint.",
            UpstreamErrorKind::RateLimited => "The upstream provider rate-limited the request.",
        }
    }

    pub fn default_hint(self) -> Option<&'static str> {
        match self {
            UpstreamErrorKind::Tls => Some(
                "Check certificate trust, hostname/SAN, and whether upstream_base_url uses the correct scheme. For trusted local providers with self-signed certificates, consider using http:// inside the private network.",
            ),
            UpstreamErrorKind::Dns => Some(
                "Check upstream_base_url, provider hostname, Docker service name, and DNS resolution from inside the AI Firewall container.",
            ),
            UpstreamErrorKind::Connect => Some(
                "Check upstream_base_url, provider host/port, Docker network membership, firewall rules, and whether the provider process is listening.",
            ),
            UpstreamErrorKind::Timeout => Some(
                "Increase request_timeout_seconds or check upstream provider latency, model load time, network latency, and provider availability.",
            ),
            UpstreamErrorKind::HttpStatus => Some(
                "Check provider response details, upstream_base_url, authentication, rate limits, and OpenAI-compatible API support.",
            ),
            UpstreamErrorKind::Other => Some(
                "Check upstream_base_url and provider availability. If this is a local provider, verify the OpenAI-compatible API is enabled.",
            ),
            UpstreamErrorKind::Authentication => Some(
                "Check upstream_api_key. For local providers without authentication, use dummy, none, null, or - so no Bearer token is sent.",
            ),
            UpstreamErrorKind::NotFound => Some(
                "Check upstream_base_url. Configure the provider root URL or its /v1 base path, not /v1/chat/completions. Verify the provider exposes an OpenAI-compatible chat completions endpoint.",
            ),
            UpstreamErrorKind::RateLimited => Some(
                "The upstream provider returned 429. Reduce request rate or check provider quota.",
            ),
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
