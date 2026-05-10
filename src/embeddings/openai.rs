use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::embeddings::provider::{EmbeddingProvider, EmbeddingResult, EmbeddingUsage};
use crate::metrics::{EMBEDDING_REQUEST_DURATION_SECONDS, EMBEDDING_TIMEOUTS_TOTAL};
use crate::upstream::llm::UpstreamErrorKind;
use crate::upstream::openai_compat::{
    build_openai_compat_url, should_send_bearer_auth, OpenAiCompatEndpoint,
};
use std::error::Error;

#[derive(Clone)]
pub struct OpenAiEmbeddingProvider {
    client: Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl OpenAiEmbeddingProvider {
    pub fn new(
        base_url: String,
        api_key: String,
        model: String,
        timeout: Duration,
    ) -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );

        let client = Client::builder()
            .default_headers(headers)
            .timeout(timeout)
            .build()
            .context("failed to build embeddings reqwest client")?;

        Ok(Self {
            client,
            base_url,
            api_key,
            model,
        })
    }
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a str,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingItem>,
    #[serde(default)]
    usage: Option<EmbeddingUsageResponse>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingUsageResponse {
    #[serde(default)]
    prompt_tokens: u32,

    #[serde(default)]
    total_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct EmbeddingItem {
    embedding: Vec<f32>,
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbeddingProvider {
    async fn embed_text(&self, input: &str) -> Result<EmbeddingResult> {
        let url = build_openai_compat_url(&self.base_url, OpenAiCompatEndpoint::Embeddings)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        let start = Instant::now();

        let req = EmbeddingRequest {
            model: &self.model,
            input,
        };

        let mut request = self.client.post(url).json(&req);

        if should_send_bearer_auth(&self.api_key) {
            request = request.bearer_auth(self.api_key.trim());
        }

        let response = match request.send().await {
            Ok(resp) => resp,
            Err(e) => {
                let elapsed = start.elapsed().as_secs_f64();
                EMBEDDING_REQUEST_DURATION_SECONDS.observe(elapsed);

                let kind = classify_reqwest_error(&e);

                if kind == UpstreamErrorKind::Timeout {
                    EMBEDDING_TIMEOUTS_TOTAL.inc();
                }

                tracing::error!(
                    error_class = kind.as_str(),
                    embedding_base_url = %self.base_url,
                    embedding_model = %self.model,
                    error = %e,
                    "embedding provider request failed"
                );

                anyhow::bail!(
                    "{} Embedding model: '{}'. Embedding provider: '{}'.",
                    match kind {
                        UpstreamErrorKind::Timeout =>
                            "Embedding provider did not respond before the configured timeout.",
                        UpstreamErrorKind::Tls =>
                            "Embedding provider TLS certificate verification failed.",
                        UpstreamErrorKind::Dns => "Failed to resolve embedding provider hostname.",
                        UpstreamErrorKind::Connect => "Failed to connect to embedding provider.",
                        _ => "Embedding provider request failed.",
                    },
                    self.model,
                    self.base_url
                );
            }
        };

        let status = response.status();
        let body = response.text().await.with_context(|| {
            format!(
                "failed reading embedding response body for model '{}' at '{}'",
                self.model, self.base_url
            )
        })?;

        let elapsed = start.elapsed().as_secs_f64();
        EMBEDDING_REQUEST_DURATION_SECONDS.observe(elapsed);

        if !status.is_success() {
            let kind = match status.as_u16() {
                401 | 403 => UpstreamErrorKind::Authentication,
                404 => UpstreamErrorKind::NotFound,
                408 | 504 => UpstreamErrorKind::Timeout,
                429 => UpstreamErrorKind::RateLimited,
                _ => UpstreamErrorKind::HttpStatus,
            };

            if kind == UpstreamErrorKind::Timeout {
                EMBEDDING_TIMEOUTS_TOTAL.inc();
            }

            tracing::error!(
                error_class = kind.as_str(),
                embedding_status = status.as_u16(),
                embedding_base_url = %self.base_url,
                embedding_model = %self.model,
                body = %body,
                "embedding provider returned an error response"
            );

            anyhow::bail!(
                "{} Status: {}. Embedding model: '{}'. Embedding provider: '{}'. Body: {}",
                embedding_status_message(kind),
                status,
                self.model,
                self.base_url,
                body
            );
        }

        let parsed: EmbeddingResponse = serde_json::from_str(&body).with_context(|| {
            format!(
                "failed to parse embedding response for model '{}' at '{}'",
                self.model, self.base_url
            )
        })?;

        let first = parsed.data.into_iter().next().context(format!(
            "embedding response contained no vectors for model '{}' at '{}'",
            self.model, self.base_url
        ))?;

        if first.embedding.is_empty() {
            anyhow::bail!(
                "embedding provider returned an empty vector for model '{}' at '{}'",
                self.model,
                self.base_url
            );
        }

        Ok(EmbeddingResult {
            embedding: first.embedding,
            usage: parsed.usage.map(|u| EmbeddingUsage {
                prompt_tokens: u.prompt_tokens,
                total_tokens: u.total_tokens,
            }),
            model: parsed.model.or_else(|| Some(self.model.clone())),
        })
    }
}

fn classify_reqwest_error(err: &reqwest::Error) -> UpstreamErrorKind {
    if err.is_timeout() {
        return UpstreamErrorKind::Timeout;
    }

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

    UpstreamErrorKind::Other
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

fn embedding_status_message(kind: UpstreamErrorKind) -> &'static str {
    match kind {
        UpstreamErrorKind::Authentication => "The embedding provider rejected authentication.",
        UpstreamErrorKind::NotFound => "The embedding endpoint was not found.",
        UpstreamErrorKind::RateLimited => "The embedding provider rate-limited the request.",
        UpstreamErrorKind::Timeout => {
            "The embedding provider did not respond before the configured timeout."
        }
        UpstreamErrorKind::HttpStatus => "The embedding provider returned an HTTP error.",
        _ => "The embedding provider request failed.",
    }
}
