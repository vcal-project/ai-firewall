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

        let mut request = self.client.post(url).json(req);

        if should_send_bearer_auth(&self.api_key) {
            request = request.bearer_auth(self.api_key.trim());
        }

        let response = match request.send().await {
            Ok(resp) => resp,

            Err(e) => {
                let elapsed = start.elapsed().as_secs_f64();
                UPSTREAM_REQUEST_DURATION_SECONDS.observe(elapsed);

                if e.is_timeout() {
                    UPSTREAM_TIMEOUTS_TOTAL.inc();

                    return Err(AppError::upstream_timeout(format!(
                        "upstream timeout for model '{}' at '{}'",
                        req.normalized_model(),
                        self.base_url
                    )));
                }

                return Err(AppError::upstream(format!(
                    "upstream request failed for model '{}' at '{}': {}",
                    req.normalized_model(),
                    self.base_url,
                    e
                )));
            }
        };

        let status = response.status();

        let body = match response.text().await {
            Ok(body) => body,
            Err(e) => {
                let elapsed = start.elapsed().as_secs_f64();
                UPSTREAM_REQUEST_DURATION_SECONDS.observe(elapsed);

                return Err(AppError::upstream(format!(
                    "failed to read upstream body: {e}"
                )));
            }
        };

        let elapsed = start.elapsed().as_secs_f64();
        UPSTREAM_REQUEST_DURATION_SECONDS.observe(elapsed);

        if !status.is_success() {
            return Err(AppError::upstream_with_status(
                status,
                format!(
                    "upstream provider returned {} for model '{}' at '{}': {}",
                    status,
                    req.normalized_model(),
                    self.base_url,
                    body
                ),
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

fn should_send_bearer_auth(api_key: &str) -> bool {
    let key = api_key.trim();

    !key.is_empty()
        && !matches!(
            key.to_ascii_lowercase().as_str(),
            "dummy" | "none" | "null" | "-"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_upstream_keys_do_not_send_auth() {
        assert!(!should_send_bearer_auth(""));
        assert!(!should_send_bearer_auth("dummy"));
        assert!(!should_send_bearer_auth("none"));
        assert!(!should_send_bearer_auth("null"));
        assert!(!should_send_bearer_auth("-"));
        assert!(!should_send_bearer_auth(" DUMMY "));
        assert!(!should_send_bearer_auth(" None "));
        assert!(should_send_bearer_auth("sk-real-key"));
    }
}
