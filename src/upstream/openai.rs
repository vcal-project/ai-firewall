use crate::error::AppError;
use crate::metrics::{UPSTREAM_CALLS, UPSTREAM_REQUEST_DURATION_SECONDS, UPSTREAM_TIMEOUTS_TOTAL};
use crate::types::openai::{
    ChatCompletionRequest, ChatCompletionResponse, ChatCompletionWireResponse,
};
use crate::upstream::llm::{LlmUpstream, UpstreamErrorKind};
use crate::upstream::openai_compat::should_send_bearer_auth;

use async_trait::async_trait;
use reqwest::{header, Client};
use std::error::Error;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct OpenAiUpstream {
    client: Client,
    base_url: String,
    api_key: String,
}

impl OpenAiUpstream {
    pub fn new(base_url: String, api_key: String, timeout: Duration) -> Result<Self, AppError> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );

        let client = Client::builder()
            .default_headers(headers)
            .timeout(timeout)
            .build()
            .map_err(|e| AppError::internal(format!("failed to build reqwest client: {e}")))?;

        Ok(Self {
            client,
            base_url,
            api_key,
        })
    }
}

#[async_trait]
impl LlmUpstream for OpenAiUpstream {
    async fn chat_completion(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, AppError> {
        let url = crate::upstream::openai_compat::build_openai_compat_url(
            &self.base_url,
            crate::upstream::openai_compat::OpenAiCompatEndpoint::ChatCompletions,
        )?;

        let start = Instant::now();

        UPSTREAM_CALLS.inc();

        let mut request = self.client.post(url).json(req);

        if should_send_bearer_auth(&self.api_key) {
            request = request.bearer_auth(self.api_key.trim());
        }

        let response = match request.send().await {
            Ok(resp) => resp,

            Err(e) => {
                let elapsed = start.elapsed().as_secs_f64();
                UPSTREAM_REQUEST_DURATION_SECONDS.observe(elapsed);

                let kind = classify_reqwest_error(&e);

                if kind == UpstreamErrorKind::Timeout {
                    UPSTREAM_TIMEOUTS_TOTAL.inc();
                }

                tracing::error!(
                    error_class = kind.as_str(),
                    upstream_base_url = %self.base_url,
                    model = %req.normalized_model(),
                    error = %e,
                    "upstream request failed"
                );

                return Err(AppError::upstream_kind(
                    kind,
                    format!(
                        "{} Model: '{}'. Upstream: '{}'.",
                        kind.default_message(),
                        req.normalized_model(),
                        self.base_url
                    ),
                ));
            }
        };

        let status = response.status();

        let body = match response.text().await {
            Ok(body) => body,
            Err(e) => {
                let elapsed = start.elapsed().as_secs_f64();
                UPSTREAM_REQUEST_DURATION_SECONDS.observe(elapsed);

                tracing::error!(
                    error_class = UpstreamErrorKind::Other.as_str(),
                    upstream_base_url = %self.base_url,
                    model = %req.normalized_model(),
                    error = %e,
                    "failed to read upstream response body"
                );

                return Err(AppError::upstream_kind(
                    UpstreamErrorKind::Other,
                    format!(
                        "Failed to read upstream response body. Model: '{}'. Upstream: '{}'.",
                        req.normalized_model(),
                        self.base_url
                    ),
                ));
            }
        };

        let elapsed = start.elapsed().as_secs_f64();
        UPSTREAM_REQUEST_DURATION_SECONDS.observe(elapsed);

        if !status.is_success() {
            let kind = classify_upstream_status(status);

            if kind == UpstreamErrorKind::Timeout {
                UPSTREAM_TIMEOUTS_TOTAL.inc();
            }

            tracing::error!(
                error_class = kind.as_str(),
                upstream_status = status.as_u16(),
                upstream_base_url = %self.base_url,
                model = %req.normalized_model(),
                response_bytes = body.len(),
                "upstream provider returned an error response"
            );

            return Err(AppError::upstream_kind_with_status(
                status,
                kind,
                format!(
                    "{} Status: {}. Model: '{}'. Upstream: '{}'.",
                    kind.default_message(),
                    status,
                    req.normalized_model(),
                    self.base_url
                ),
            ));
        }

        let parsed = serde_json::from_str::<ChatCompletionWireResponse>(&body)
            .map_err(|e| {
                tracing::error!(
                    error_class = UpstreamErrorKind::Other.as_str(),
                    upstream_base_url = %self.base_url,
                    model = %req.normalized_model(),
                    response_bytes = body.len(),
                    error = %e,
                    "failed to parse upstream response body"
                );

                AppError::upstream_kind(
                    UpstreamErrorKind::Other,
                    format!(
                        "Failed to parse upstream response body for model '{}' at '{}'. Verify the provider returns a non-streaming OpenAI-compatible /v1/chat/completions JSON response.",
                        req.normalized_model(),
                        self.base_url
                    ),
                )
            })?
            .into_normalized(req.normalized_model(), &self.base_url)
            .map_err(|e| {
                tracing::error!(
                    error_class = UpstreamErrorKind::Other.as_str(),
                    upstream_base_url = %self.base_url,
                    model = %req.normalized_model(),
                    error = %e,
                    "upstream response failed OpenAI-compatible normalization"
                );

                AppError::upstream_kind(
                    UpstreamErrorKind::Other,
                    format!(
                        "Upstream response for model '{}' at '{}' is not a usable OpenAI-compatible chat response: {}. Verify the provider response includes a usable choices array and OpenAI-compatible chat completion fields.",
                        req.normalized_model(),
                        self.base_url,
                        e
                    ),
                )
            })?;

        Ok(parsed)
    }
}

fn classify_reqwest_error(err: &reqwest::Error) -> UpstreamErrorKind {
    if err.is_timeout() {
        return UpstreamErrorKind::Timeout;
    }

    // TLS/certificate errors are often wrapped as connect errors,
    // so check the full error chain before returning Connect.
    if error_chain_contains(
        err,
        &[
            "certificate",
            "cert",
            "tls",
            "ssl",
            "unknown issuer",
            "invalid peer certificate",
            "self-signed",
            "hostname",
            "not valid for name",
            "subject alternative name",
            "invalid certificate",
            "certificate verify failed",
        ],
    ) {
        return UpstreamErrorKind::Tls;
    }

    if error_chain_contains(
        err,
        &[
            "dns",
            "failed to lookup address",
            "name or service not known",
            "temporary failure in name resolution",
            "no such host",
        ],
    ) {
        return UpstreamErrorKind::Dns;
    }

    if err.is_connect() {
        return UpstreamErrorKind::Connect;
    }

    if err.status().is_some() {
        return UpstreamErrorKind::HttpStatus;
    }

    UpstreamErrorKind::Other
}

fn classify_upstream_status(status: reqwest::StatusCode) -> UpstreamErrorKind {
    match status.as_u16() {
        401 | 403 => UpstreamErrorKind::Authentication,
        404 => UpstreamErrorKind::NotFound,
        408 | 504 => UpstreamErrorKind::Timeout,
        429 => UpstreamErrorKind::RateLimited,
        _ => UpstreamErrorKind::HttpStatus,
    }
}

fn error_chain_contains(err: &(dyn Error + 'static), needles: &[&str]) -> bool {
    let mut current: Option<&(dyn Error + 'static)> = Some(err);

    while let Some(e) = current {
        let msg = e.to_string().to_lowercase();

        if needles.iter().any(|needle| msg.contains(needle)) {
            return true;
        }

        current = e.source();
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_upstream_http_statuses() {
        assert_eq!(
            classify_upstream_status(reqwest::StatusCode::UNAUTHORIZED),
            UpstreamErrorKind::Authentication
        );
        assert_eq!(
            classify_upstream_status(reqwest::StatusCode::FORBIDDEN),
            UpstreamErrorKind::Authentication
        );
        assert_eq!(
            classify_upstream_status(reqwest::StatusCode::NOT_FOUND),
            UpstreamErrorKind::NotFound
        );
        assert_eq!(
            classify_upstream_status(reqwest::StatusCode::TOO_MANY_REQUESTS),
            UpstreamErrorKind::RateLimited
        );
        assert_eq!(
            classify_upstream_status(reqwest::StatusCode::GATEWAY_TIMEOUT),
            UpstreamErrorKind::Timeout
        );
        assert_eq!(
            classify_upstream_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
            UpstreamErrorKind::HttpStatus
        );
    }
}
