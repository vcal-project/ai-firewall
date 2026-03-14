use crate::types::openai::{ChatCompletionRequest, ChatMessage};
use anyhow::Result;
use serde_json::{json, Value};

pub fn normalize_chat_request(req: &ChatCompletionRequest) -> Result<String> {
    let normalized = json!({
        "model": req.model.trim(),
        "messages": normalize_messages(&req.messages),
        "temperature": req.temperature.unwrap_or(1.0),
        "top_p": req.top_p.unwrap_or(1.0),
        "max_tokens": req.max_tokens,
        "stream": req.stream.unwrap_or(false),
    });

    Ok(serde_json::to_string(&normalized)?)
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
            json!({
                "role": m.role.trim(),
                "content": normalize_content(&m.content),
                "name": m.name.as_deref().map(str::trim),
            })
        })
        .collect()
}

fn normalize_content(content: &Value) -> Value {
    match content {
        Value::String(s) => Value::String(s.trim().to_string()),
        v => v.clone(),
    }
}
