use crate::types::openai::{ChatCompletionRequest, ChatMessage};
use anyhow::Result;
use serde_json::{Map, Value};

pub fn normalize_chat_request(req: &ChatCompletionRequest) -> Result<String> {
    let mut normalized = Map::new();

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

    for (key, value) in &req.extra {
        normalized
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }

    Ok(serde_json::to_string(&Value::Object(normalized))?)
}

/// Scope semantic reuse by generation settings and earlier conversation context.
/// The final user message is intentionally excluded: paraphrases can still match.
/// Versioning prevents reading legacy Qdrant entries without compatibility data.
pub fn semantic_compatibility_scope(req: &ChatCompletionRequest) -> Result<String> {
    use crate::core::hashing::sha256_hex;
    let normalized = normalize_chat_request(req)?;
    let mut identity: Value = serde_json::from_str(&normalized)?;
    let object = identity
        .as_object_mut()
        .expect("normalized request is an object");
    object.remove("model"); // Already an explicit Qdrant filter.
    object.remove("stream"); // SSE and JSON responses share the canonical completion.
    object.remove("stream_options");
    // Observability/client identity fields do not control generation and often
    // vary between Open WebUI requests. Keep unknown/provider-specific fields
    // in scope (fail closed for semantic compatibility).
    object.remove("metadata");
    object.remove("user");
    object.remove("store");
    object.remove("messages");
    // System/developer instructions and previous turns must agree. Only a final
    // user message may vary for semantic matching.
    let mut context = req.messages.clone();
    if context
        .last()
        .is_some_and(|message| message.role.trim() == "user")
    {
        context.pop();
    }
    object.insert("context".into(), Value::Array(normalize_messages(&context)));
    Ok(format!(
        "semantic-v2:{}",
        sha256_hex(&serde_json::to_string(&identity)?)
    ))
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
    fn semantic_scope_distinguishes_generation_limits_but_allows_paraphrases() {
        let mut first = request();
        first.max_tokens = Some(160);
        let mut limit_changed = first.clone();
        limit_changed.max_tokens = Some(1024);
        assert_ne!(
            semantic_compatibility_scope(&first).unwrap(),
            semantic_compatibility_scope(&limit_changed).unwrap()
        );

        let mut paraphrase = first.clone();
        paraphrase.messages[0].content = serde_json::json!("different phrasing");
        assert_eq!(
            semantic_compatibility_scope(&first).unwrap(),
            semantic_compatibility_scope(&paraphrase).unwrap()
        );

        let mut metadata_changed = first.clone();
        metadata_changed.extra.insert(
            "metadata".into(),
            serde_json::json!({"request_id":"changing"}),
        );
        assert_eq!(
            semantic_compatibility_scope(&first).unwrap(),
            semantic_compatibility_scope(&metadata_changed).unwrap()
        );

        let mut format_changed = first.clone();
        format_changed
            .extra
            .insert("max_completion_tokens".into(), serde_json::json!(500));
        assert_ne!(
            semantic_compatibility_scope(&first).unwrap(),
            semantic_compatibility_scope(&format_changed).unwrap()
        );
    }

    #[test]
    fn semantic_scope_isolates_system_and_history() {
        let mut first = request();
        first.messages.insert(
            0,
            ChatMessage {
                role: "system".into(),
                content: serde_json::json!("Be concise"),
                name: None,
                extra: Map::new(),
            },
        );
        let mut changed = first.clone();
        changed.messages[0].content = serde_json::json!("Respond in JSON");
        assert_ne!(
            semantic_compatibility_scope(&first).unwrap(),
            semantic_compatibility_scope(&changed).unwrap()
        );
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
