use crate::error::AppError;
use crate::metrics::{UPSTREAM_CALLS, UPSTREAM_REQUEST_DURATION_SECONDS, UPSTREAM_TIMEOUTS_TOTAL};
use crate::types::openai::{ChatCompletionRequest, ChatCompletionResponse};
use crate::upstream::llm::LlmUpstream;

use async_trait::async_trait;
use reqwest::{header, Client};
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
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );

        let start = Instant::now();

        UPSTREAM_CALLS.inc();

        let response = match self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(req)
            .send()
            .await
        {
            Ok(resp) => resp,

            Err(e) => {
                let elapsed = start.elapsed().as_secs_f64();
                UPSTREAM_REQUEST_DURATION_SECONDS.observe(elapsed);

                if e.is_timeout() {
                    UPSTREAM_TIMEOUTS_TOTAL.inc();

                    return Err(AppError::upstream_timeout(format!(
                        "upstream timeout for model '{}'",
                        req.normalized_model()
                    )));
                }

                return Err(AppError::upstream(format!(
                    "upstream request failed: {}",
                    e
                )));
            }
        };

        let status = response.status();

        let body = response
            .text()
            .await
            .map_err(|e| AppError::upstream(format!("failed to read upstream body: {e}")))?;

        let elapsed = start.elapsed().as_secs_f64();
        UPSTREAM_REQUEST_DURATION_SECONDS.observe(elapsed);

        if !status.is_success() {
            return Err(AppError::upstream_with_status(
                status,
                format!("upstream returned {} with body: {}", status, body),
            ));
        }

        let parsed = serde_json::from_str::<ChatCompletionResponse>(&body).map_err(|e| {
            AppError::upstream(format!(
                "failed to parse upstream response body: {e}; body: {}",
                body
            ))
        })?;

        Ok(parsed)
    }
}
