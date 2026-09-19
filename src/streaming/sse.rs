use axum::body::Bytes;
use serde_json::{Map, Value};

use crate::types::openai::{ChatCompletionRequest, ChatCompletionResponse};

pub fn stream_include_usage(request: &ChatCompletionRequest) -> bool {
    request
        .extra
        .get("stream_options")
        .and_then(Value::as_object)
        .and_then(|options| options.get("include_usage"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub fn encode_response_as_sse(response: &ChatCompletionResponse, include_usage: bool) -> Bytes {
    let mut payload = String::new();

    for choice in &response.choices {
        let mut delta = Map::new();
        delta.insert(
            "role".to_string(),
            Value::String(choice.message.role.clone()),
        );

        if !choice.message.content.is_null() {
            delta.insert("content".to_string(), choice.message.content.clone());
        }

        if let Some(name) = &choice.message.name {
            delta.insert("name".to_string(), Value::String(name.clone()));
        }

        for (key, value) in &choice.message.extra {
            if key == "tool_calls" {
                delta.insert(key.clone(), tool_calls_for_stream(value));
            } else {
                delta.insert(key.clone(), value.clone());
            }
        }

        let mut choice_value = Map::new();
        choice_value.insert("index".to_string(), Value::from(choice.index));
        choice_value.insert("delta".to_string(), Value::Object(delta));
        choice_value.insert("finish_reason".to_string(), Value::Null);

        for (key, value) in &choice.extra {
            choice_value.insert(key.clone(), value.clone());
        }

        append_event(
            &mut payload,
            chunk_value(response, vec![Value::Object(choice_value)], None),
        );
    }

    let final_choices = response
        .choices
        .iter()
        .map(|choice| {
            serde_json::json!({
                "index": choice.index,
                "delta": {},
                "finish_reason": choice.finish_reason
            })
        })
        .collect::<Vec<_>>();

    append_event(&mut payload, chunk_value(response, final_choices, None));

    if include_usage {
        if let Some(usage) = &response.usage {
            append_event(
                &mut payload,
                chunk_value(
                    response,
                    Vec::new(),
                    Some(serde_json::to_value(usage).unwrap_or(Value::Null)),
                ),
            );
        }
    }

    payload.push_str("data: [DONE]\n\n");
    Bytes::from(payload)
}

fn tool_calls_for_stream(value: &Value) -> Value {
    let Some(tool_calls) = value.as_array() else {
        return value.clone();
    };

    Value::Array(
        tool_calls
            .iter()
            .enumerate()
            .map(|(index, tool_call)| {
                let Some(object) = tool_call.as_object() else {
                    return tool_call.clone();
                };

                let mut object = object.clone();
                object.insert("index".to_string(), Value::from(index as u64));
                Value::Object(object)
            })
            .collect(),
    )
}

fn chunk_value(
    response: &ChatCompletionResponse,
    choices: Vec<Value>,
    usage: Option<Value>,
) -> Value {
    let mut object = Map::new();
    object.insert("id".to_string(), Value::String(response.id.clone()));
    object.insert(
        "object".to_string(),
        Value::String("chat.completion.chunk".to_string()),
    );
    object.insert("created".to_string(), Value::from(response.created));
    object.insert("model".to_string(), Value::String(response.model.clone()));
    object.insert("choices".to_string(), Value::Array(choices));

    for (key, value) in &response.extra {
        object.insert(key.clone(), value.clone());
    }

    if let Some(usage) = usage {
        object.insert("usage".to_string(), usage);
    }

    Value::Object(object)
}

fn append_event(payload: &mut String, value: Value) {
    payload.push_str("data: ");
    match serde_json::to_string(&value) {
        Ok(encoded) => payload.push_str(&encoded),
        Err(_) => payload.push_str("{}"),
    }
    payload.push_str("\n\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::openai::{ChatMessage, Choice, Usage};

    #[test]
    fn encodes_canonical_response_as_valid_sse() {
        let response = ChatCompletionResponse {
            id: "abc".into(),
            object: "chat.completion".into(),
            created: 123,
            model: "m".into(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".into(),
                    content: Value::String("hello".into()),
                    name: None,
                    extra: {
                        let mut extra = Map::new();
                        extra.insert(
                            "tool_calls".into(),
                            serde_json::json!([{
                                "id": "call_1",
                                "type": "function",
                                "function": {"name": "lookup", "arguments": "{}"}
                            }]),
                        );
                        extra
                    },
                },
                finish_reason: Some("stop".into()),
                extra: Map::new(),
            }],
            usage: Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                extra: Map::new(),
            }),
            extra: Map::new(),
        };

        let text = String::from_utf8(encode_response_as_sse(&response, true).to_vec()).unwrap();
        assert!(text.contains("\"content\":\"hello\""));
        assert!(text.contains("\"finish_reason\":\"stop\""));
        assert!(text.contains("\"total_tokens\":15"));
        let first = text
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("first SSE data event");
        let first: Value = serde_json::from_str(first).unwrap();
        assert_eq!(first["choices"][0]["delta"]["tool_calls"][0]["index"], 0);
        assert!(text.ends_with("data: [DONE]\n\n"));
    }
}
