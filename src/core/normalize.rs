use crate::types::openai::{ChatCompletionRequest, ChatMessage};
use anyhow::Result;
use serde_json::{Map, Value};

pub fn normalize_chat_request(req: &ChatCompletionRequest) -> Result<String> {
    let mut normalized = Map::new();

    // Keep the existing cache-equivalence rules for common OpenAI parameters:
    // omitted temperature/top_p are equivalent to their protocol defaults and
    // stream is a delivery concern rather than completion identity.
    normalized.insert(
        "model".to_string(),
        Value::String(req.model.trim().to_string()),
    );
    normalized.insert(
        "messages".to_string(),
        Value::Array(normalize_messages(&req.messages)),
    );
    normalized.insert(
        "temperature".to_string(),
        serde_json::json!(req.temperature.unwrap_or(1.0)),
    );
    normalized.insert(
        "top_p".to_string(),
        serde_json::json!(req.top_p.unwrap_or(1.0)),
    );
    normalized.insert(
        "max_tokens".to_string(),
        req.max_tokens.map(Value::from).unwrap_or(Value::Null),
    );
    normalized.insert(
        "stream".to_string(),
        Value::Bool(req.stream.unwrap_or(false)),
    );

    // ChatCompletionRequest intentionally flattens unknown/OpenAI-compatible
    // fields so AIF can proxy newer parameters without a release. They are also
    // part of completion identity unless the caller removes a known
    // delivery-only field before normalization (for example stream_options).
    for (key, value) in &req.extra {
        normalized
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }

    Ok(serde_json::to_string(&Value::Object(normalized))?)
}

pub fn semantic_text_from_request(req: &ChatCompletionRequest) -> String {
    req.messages
        .iter()
        .map(|m| {
            let content = match &m.content {
                Value::String(s) => s.trim().to_string(),
                other => other.to_string(),
            };

            match &m.name {
                Some(name) if !name.trim().is_empty() => {
                    format!("{}({}): {}", m.role.trim(), name.trim(), content)
                }
                _ => format!("{}: {}", m.role.trim(), content),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_messages(messages: &[ChatMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| {
            let mut normalized = Map::new();
            normalized.insert("role".to_string(), Value::String(m.role.trim().to_string()));
            normalized.insert("content".to_string(), normalize_content(&m.content));
            normalized.insert(
                "name".to_string(),
                m.name
                    .as_deref()
                    .map(str::trim)
                    .map(|name| Value::String(name.to_string()))
                    .unwrap_or(Value::Null),
            );

            // Preserve tool-call IDs, tool calls, provider extensions, and any
            // future OpenAI-compatible message fields in exact-cache identity.
            for (key, value) in &m.extra {
                normalized
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }

            Value::Object(normalized)
        })
        .collect()
}

fn normalize_content(content: &Value) -> Value {
    match content {
        Value::String(s) => Value::String(s.trim().to_string()),
        v => v.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::openai::ChatMessage;

    fn request() -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: " demo-model ".to_string(),
            messages: vec![ChatMessage {
                role: " user ".to_string(),
                content: serde_json::json!(" hello "),
                name: None,
                extra: Map::new(),
            }],
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        }
    }

    #[test]
    fn exact_cache_identity_includes_top_level_extra_fields() {
        let mut first = request();
        first.extra.insert(
            "tools".to_string(),
            serde_json::json!([{
                "type": "function",
                "function": {"name": "lookup_a", "parameters": {"type": "object"}}
            }]),
        );

        let mut second = first.clone();
        second.extra.insert(
            "tools".to_string(),
            serde_json::json!([{
                "type": "function",
                "function": {"name": "lookup_b", "parameters": {"type": "object"}}
            }]),
        );

        assert_ne!(
            normalize_chat_request(&first).unwrap(),
            normalize_chat_request(&second).unwrap()
        );
    }

    #[test]
    fn exact_cache_identity_includes_message_extra_fields() {
        let mut first = request();
        first.messages[0]
            .extra
            .insert("tool_call_id".to_string(), Value::String("call_a".into()));

        let mut second = first.clone();
        second.messages[0]
            .extra
            .insert("tool_call_id".to_string(), Value::String("call_b".into()));

        assert_ne!(
            normalize_chat_request(&first).unwrap(),
            normalize_chat_request(&second).unwrap()
        );
    }

    #[test]
    fn exact_cache_identity_keeps_existing_normalization_rules() {
        let first = request();
        let mut second = request();
        second.model = "demo-model".to_string();
        second.messages[0].role = "user".to_string();
        second.messages[0].content = serde_json::json!("hello");
        second.temperature = Some(1.0);
        second.top_p = Some(1.0);
        second.stream = Some(false);

        assert_eq!(
            normalize_chat_request(&first).unwrap(),
            normalize_chat_request(&second).unwrap()
        );
    }
}
