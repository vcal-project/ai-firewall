use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::embeddings::provider::{EmbeddingProvider, EmbeddingResult, EmbeddingUsage};

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
    prompt_tokens: u32,
    total_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct EmbeddingItem {
    embedding: Vec<f32>,
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbeddingProvider {
    async fn embed_text(&self, input: &str) -> Result<EmbeddingResult> {
        let url = format!("{}/v1/embeddings", self.base_url.trim_end_matches('/'));

        let req = EmbeddingRequest {
            model: &self.model,
            input,
        };

        let response = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&req)
            .send()
            .await
            .context("embedding request failed")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed reading embedding body")?;

        if !status.is_success() {
            anyhow::bail!("embedding upstream returned {}: {}", status, body);
        }

        let parsed: EmbeddingResponse =
            serde_json::from_str(&body).context("failed to parse embedding response")?;

        let first = parsed
            .data
            .into_iter()
            .next()
            .context("embedding response contained no vectors")?;

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
