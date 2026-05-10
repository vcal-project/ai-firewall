use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: Value,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionWireResponse {
    #[serde(default)]
    pub id: Option<String>,

    #[serde(default)]
    pub object: Option<String>,

    #[serde(default)]
    pub created: Option<i64>,

    #[serde(default)]
    pub model: Option<String>,

    #[serde(default)]
    pub choices: Vec<Choice>,

    #[serde(default)]
    pub usage: Option<Usage>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl ChatCompletionWireResponse {
    pub fn into_normalized(
        self,
        requested_model: &str,
        upstream_base_url: &str,
    ) -> Result<ChatCompletionResponse, String> {
        if self.choices.is_empty() {
            return Err("upstream response contained no choices".to_string());
        }

        let model = match self.model {
            Some(model) if !model.trim().is_empty() => model,
            _ => {
                tracing::warn!(
                    requested_model = %requested_model,
                    upstream_base_url = %upstream_base_url,
                    "upstream response missing model; using requested model"
                );

                requested_model.to_string()
            }
        };

        let mut usage = self.usage;

        if let Some(u) = usage.as_mut() {
            if u.total_tokens == 0 {
                u.total_tokens = u.prompt_tokens.saturating_add(u.completion_tokens);
            }
        }

        Ok(ChatCompletionResponse {
            id: self
                .id
                .unwrap_or_else(|| "chatcmpl-openai-compatible".to_string()),
            object: self.object.unwrap_or_else(|| "chat.completion".to_string()),
            created: self
                .created
                .unwrap_or_else(|| chrono::Utc::now().timestamp()),
            model,
            choices: self.choices,
            usage,
            extra: self.extra,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: ChatMessage,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u32,

    #[serde(default)]
    pub completion_tokens: u32,

    #[serde(default)]
    pub total_tokens: u32,
}

impl ChatCompletionRequest {
    pub fn normalized_model(&self) -> &str {
        self.model.trim()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_response_missing_model() {
        let body = r#"
        {
          "id": "abc",
          "object": "chat.completion",
          "created": 123,
          "choices": [
            {
              "index": 0,
              "message": {
                "role": "assistant",
                "content": "hello",
                "name": null
              },
              "finish_reason": "stop"
            }
          ]
        }
        "#;

        let wire: ChatCompletionWireResponse = serde_json::from_str(body).unwrap();
        let normalized = wire
            .into_normalized("local-model", "http://localhost:11434/v1")
            .unwrap();

        assert_eq!(normalized.model, "local-model");
        assert_eq!(normalized.choices.len(), 1);
    }

    #[test]
    fn normalizes_partial_usage() {
        let body = r#"
        {
          "id": "abc",
          "object": "chat.completion",
          "created": 123,
          "model": "local-model",
          "choices": [
            {
              "index": 0,
              "message": {
                "role": "assistant",
                "content": "hello",
                "name": null
              },
              "finish_reason": "stop"
            }
          ],
          "usage": {
            "prompt_tokens": 10
          }
        }
        "#;

        let wire: ChatCompletionWireResponse = serde_json::from_str(body).unwrap();
        let normalized = wire
            .into_normalized("local-model", "http://localhost:11434/v1")
            .unwrap();

        let usage = normalized.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 0);
        assert_eq!(usage.total_tokens, 10);
    }

    #[test]
    fn rejects_response_without_choices() {
        let body = r#"
        {
          "id": "abc",
          "object": "chat.completion",
          "created": 123,
          "model": "local-model",
          "choices": []
        }
        "#;

        let wire: ChatCompletionWireResponse = serde_json::from_str(body).unwrap();
        let err = wire
            .into_normalized("local-model", "http://localhost:11434/v1")
            .unwrap_err();

        assert!(err.contains("no choices"));
    }
}
