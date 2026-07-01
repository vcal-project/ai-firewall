use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,

    #[serde(default)]
    pub content: Value,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
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

    #[serde(default, flatten)]
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

    #[serde(default, flatten)]
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

    #[serde(default, flatten)]
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

    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u32,

    #[serde(default)]
    pub completion_tokens: u32,

    #[serde(default)]
    pub total_tokens: u32,

    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
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

    #[test]
    fn preserves_request_message_extra_fields() {
        let body = r#"
        {
          "model": "gpt-4o-mini",
          "messages": [
            {
              "role": "tool",
              "content": "tool result",
              "tool_call_id": "call_123",
              "provider_metadata": {"trace_id": "abc"}
            }
          ]
        }
        "#;

        let request: ChatCompletionRequest = serde_json::from_str(body).unwrap();
        let message = &request.messages[0];

        assert_eq!(
            message.extra.get("tool_call_id"),
            Some(&Value::String("call_123".to_string()))
        );
        assert_eq!(
            message.extra.get("provider_metadata"),
            Some(&serde_json::json!({"trace_id": "abc"}))
        );

        let serialized = serde_json::to_value(&request).unwrap();
        assert_eq!(
            serialized["messages"][0]["tool_call_id"],
            Value::String("call_123".to_string())
        );
        assert_eq!(
            serialized["messages"][0]["provider_metadata"],
            serde_json::json!({"trace_id": "abc"})
        );
    }

    #[test]
    fn preserves_response_message_extra_fields_after_normalization() {
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
                "tool_calls": [
                  {
                    "id": "call_123",
                    "type": "function",
                    "function": {"name": "lookup", "arguments": "{}"}
                  }
                ],
                "provider_metadata": {"trace_id": "abc"}
              },
              "finish_reason": "tool_calls"
            }
          ]
        }
        "#;

        let wire: ChatCompletionWireResponse = serde_json::from_str(body).unwrap();
        let normalized = wire
            .into_normalized("local-model", "http://localhost:11434/v1")
            .unwrap();

        let message = &normalized.choices[0].message;
        assert_eq!(
            message.extra.get("tool_calls"),
            Some(&serde_json::json!([
                {
                    "id": "call_123",
                    "type": "function",
                    "function": {"name": "lookup", "arguments": "{}"}
                }
            ]))
        );
        assert_eq!(
            message.extra.get("provider_metadata"),
            Some(&serde_json::json!({"trace_id": "abc"}))
        );

        let serialized = serde_json::to_value(&normalized).unwrap();
        assert_eq!(
            serialized["choices"][0]["message"]["tool_calls"],
            serde_json::json!([
                {
                    "id": "call_123",
                    "type": "function",
                    "function": {"name": "lookup", "arguments": "{}"}
                }
            ])
        );
        assert_eq!(
            serialized["choices"][0]["message"]["provider_metadata"],
            serde_json::json!({"trace_id": "abc"})
        );
    }
    #[test]
    fn preserves_request_extra_fields_and_non_string_content() {
        let body = r#"
        {
          "model": "gpt-4o-mini",
          "messages": [
            {
              "role": "user",
              "content": [
                {"type": "text", "text": "hello john@example.com"},
                {"type": "image_url", "image_url": {"url": "https://example.com/image.png"}}
              ]
            }
          ],
          "temperature": 0.2,
          "tools": [
            {
              "type": "function",
              "function": {"name": "lookup", "parameters": {"type": "object"}}
            }
          ],
          "tool_choice": "auto",
          "metadata": {"tenant": "demo"},
          "response_format": {"type": "json_object"},
          "reasoning_effort": "low"
        }
        "#;

        let request: ChatCompletionRequest = serde_json::from_str(body).unwrap();

        assert!(request.messages[0].content.is_array());
        assert_eq!(
            request.extra.get("tools"),
            Some(&serde_json::json!([
                {
                    "type": "function",
                    "function": {"name": "lookup", "parameters": {"type": "object"}}
                }
            ]))
        );
        assert_eq!(
            request.extra.get("tool_choice"),
            Some(&Value::String("auto".to_string()))
        );
        assert_eq!(
            request.extra.get("metadata"),
            Some(&serde_json::json!({"tenant": "demo"}))
        );
        assert_eq!(
            request.extra.get("response_format"),
            Some(&serde_json::json!({"type": "json_object"}))
        );
        assert_eq!(
            request.extra.get("reasoning_effort"),
            Some(&Value::String("low".to_string()))
        );

        let serialized = serde_json::to_value(&request).unwrap();
        assert_eq!(
            serialized["messages"][0]["content"],
            request.messages[0].content
        );
        assert_eq!(serialized["tools"], request.extra["tools"]);
        assert_eq!(serialized["reasoning_effort"], "low");
    }

    #[test]
    fn preserves_choice_and_usage_extra_fields() {
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
                "content": "hello"
              },
              "finish_reason": "stop",
              "logprobs": {"content": []},
              "provider_choice_metadata": {"rank": 1}
            }
          ],
          "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 15,
            "prompt_tokens_details": {"cached_tokens": 3}
          }
        }
        "#;

        let wire: ChatCompletionWireResponse = serde_json::from_str(body).unwrap();
        let normalized = wire
            .into_normalized("local-model", "http://localhost:11434/v1")
            .unwrap();

        let choice = &normalized.choices[0];
        assert_eq!(
            choice.extra.get("logprobs"),
            Some(&serde_json::json!({"content": []}))
        );
        assert_eq!(
            choice.extra.get("provider_choice_metadata"),
            Some(&serde_json::json!({"rank": 1}))
        );

        let usage = normalized.usage.unwrap();
        assert_eq!(
            usage.extra.get("prompt_tokens_details"),
            Some(&serde_json::json!({"cached_tokens": 3}))
        );
    }

    #[test]
    fn accepts_messages_without_content() {
        let body = r#"
        {
          "model": "gpt-4o-mini",
          "messages": [
            {
              "role": "assistant",
              "tool_calls": [
                {
                  "id": "call_123",
                  "type": "function",
                  "function": {"name": "lookup", "arguments": "{}"}
                }
              ]
            }
          ]
        }
        "#;

        let request: ChatCompletionRequest = serde_json::from_str(body).unwrap();

        assert!(request.messages[0].content.is_null());
        assert_eq!(
            request.messages[0].extra.get("tool_calls"),
            Some(&serde_json::json!([
                {
                  "id": "call_123",
                  "type": "function",
                  "function": {"name": "lookup", "arguments": "{}"}
                }
            ]))
        );
    }
}
