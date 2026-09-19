use async_trait::async_trait;
use axum::body::Bytes;
use futures_util::Stream;
use std::{error::Error, pin::Pin, time::Duration};

use crate::error::AppError;
use crate::types::openai::{ChatCompletionRequest, ChatCompletionResponse};

pub type UpstreamStreamError = Box<dyn Error + Send + Sync>;

/// Default absolute ceiling for one provider-side controlled stream.
/// Provider implementations may override this when they have a stronger bound.
pub const DEFAULT_MAX_STREAM_GENERATION_DURATION: Duration = Duration::from_secs(15 * 60);

pub type UpstreamByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, UpstreamStreamError>> + Send + 'static>>;

pub struct UpstreamStreamResponse {
    pub status: reqwest::StatusCode,
    pub body: UpstreamByteStream,
}

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
            UpstreamErrorKind::Tls => {
                "TLS/certificate verification failed while contacting the upstream provider."
            }
            UpstreamErrorKind::Dns => "Failed to resolve the upstream provider hostname.",
            UpstreamErrorKind::Connect => {
                "Failed to connect to the upstream provider host or port."
            }
            UpstreamErrorKind::HttpStatus => {
                "The upstream provider returned an HTTP error response."
            }
            UpstreamErrorKind::Other => {
                "The upstream provider request failed before a valid response was received."
            }
            UpstreamErrorKind::Authentication => "The upstream provider rejected authentication.",
            UpstreamErrorKind::NotFound => {
                "The upstream provider returned 404 for the OpenAI-compatible endpoint."
            }
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
                "Increase upstream_timeout_seconds or check upstream provider latency, model load time, network latency, and provider availability.",
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
    /// Maximum idle interval between provider-side streaming body chunks.
    /// OpenAI-compatible HTTP implementations should normally return their
    /// configured upstream timeout here.
    fn stream_idle_timeout(&self) -> Duration {
        Duration::from_secs(120)
    }

    /// Absolute ceiling for provider-side streaming generation, independent of
    /// chunk activity. This prevents a malicious or broken drip-feed stream from
    /// holding an upstream concurrency permit indefinitely.
    fn max_stream_generation_duration(&self) -> Duration {
        DEFAULT_MAX_STREAM_GENERATION_DURATION
    }

    async fn chat_completion(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, AppError>;

    /// Opens the provider-side streaming transport. AI Firewall consumes this
    /// stream internally and does not expose these bytes directly to clients.
    async fn chat_completion_stream(
        &self,
        _req: &ChatCompletionRequest,
    ) -> Result<UpstreamStreamResponse, AppError> {
        Err(AppError::unprocessable(
            "stream=true is not supported by the configured upstream implementation",
        ))
    }
}
