use crate::types::openai::{ChatCompletionRequest, ChatCompletionResponse};
use crate::upstream::llm::LlmUpstream;
use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::{header, Client};
use std::time::Duration;

#[derive(Clone)]
pub struct OpenAiUpstream {
    client: Client,
    base_url: String,
    api_key: String,
}

impl OpenAiUpstream {
    pub fn new(base_url: String, api_key: String, timeout: Duration) -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );

        let client = Client::builder()
            .default_headers(headers)
            .timeout(timeout)
            .build()
            .context("failed to build reqwest client")?;

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
    ) -> anyhow::Result<ChatCompletionResponse> {
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );

        let response = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(req)
            .send()
            .await
            .context("upstream request failed")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read upstream body")?;

        if !status.is_success() {
            anyhow::bail!("upstream returned {}: {}", status, body);
        }

        let parsed = serde_json::from_str::<ChatCompletionResponse>(&body)
            .context("failed to parse upstream response")?;

        Ok(parsed)
    }
}
