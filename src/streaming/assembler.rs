use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::types::openai::{ChatCompletionResponse, ChatMessage, Choice, Usage};

#[derive(Debug, Default)]
struct ChoiceAccumulator {
    role: Option<String>,
    name: Option<String>,
    content: Value,
    saw_content: bool,
    finish_reason: Option<String>,
    message_extra: Map<String, Value>,
    choice_extra: Map<String, Value>,
}

impl ChoiceAccumulator {
    fn push_content(&mut self, value: &Value) {
        match value {
            Value::Null => {}
            Value::String(fragment) => {
                self.saw_content = true;
                match &mut self.content {
                    Value::String(current) => current.push_str(fragment),
                    Value::Null => self.content = Value::String(fragment.clone()),
                    _ => self.content = Value::String(fragment.clone()),
                }
            }
            Value::Array(fragment) => {
                self.saw_content = true;
                match &mut self.content {
                    Value::Array(current) => current.extend(fragment.clone()),
                    Value::Null => self.content = Value::Array(fragment.clone()),
                    _ => self.content = Value::Array(fragment.clone()),
                }
            }
            other => {
                self.saw_content = true;
                self.content = other.clone();
            }
        }
    }
}

#[derive(Debug)]
pub struct OpenAiStreamAssembler {
    requested_model: String,
    max_bytes: usize,
    received_bytes: usize,
    pending: Vec<u8>,
    saw_done: bool,
    id: Option<String>,
    created: Option<i64>,
    model: Option<String>,
    choices: BTreeMap<u32, ChoiceAccumulator>,
    usage: Option<Usage>,
    extra: Map<String, Value>,
}

impl OpenAiStreamAssembler {
    pub fn new(requested_model: impl Into<String>, max_bytes: usize) -> Self {
        Self {
            requested_model: requested_model.into(),
            max_bytes,
            received_bytes: 0,
            pending: Vec::new(),
            saw_done: false,
            id: None,
            created: None,
            model: None,
            choices: BTreeMap::new(),
            usage: None,
            extra: Map::new(),
        }
    }

    pub fn received_bytes(&self) -> usize {
        self.received_bytes
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.received_bytes = self
            .received_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| "stream response size overflow".to_string())?;

        if self.received_bytes > self.max_bytes {
            return Err(format!(
                "controlled stream exceeded maximum buffered size ({} > {} bytes)",
                self.received_bytes, self.max_bytes
            ));
        }

        self.pending.extend_from_slice(bytes);
        self.drain_events(false)
    }

    pub fn finish(mut self) -> Result<ChatCompletionResponse, String> {
        self.drain_events(true)?;

        if self.choices.is_empty() {
            return Err("upstream stream contained no completion choices".to_string());
        }

        let all_finished = self
            .choices
            .values()
            .all(|choice| choice.finish_reason.is_some());

        if !self.saw_done && !all_finished {
            return Err(
                "upstream stream ended before [DONE] or a terminal finish_reason".to_string(),
            );
        }

        let choices = self
            .choices
            .into_iter()
            .map(|(index, mut acc)| {
                if acc.message_extra.contains_key("tool_calls") {
                    strip_tool_call_indexes(&mut acc.message_extra);
                }

                let content = if acc.saw_content {
                    acc.content
                } else if acc.message_extra.contains_key("tool_calls") {
                    Value::Null
                } else {
                    Value::String(String::new())
                };

                Choice {
                    index,
                    message: ChatMessage {
                        role: acc.role.unwrap_or_else(|| "assistant".to_string()),
                        content,
                        name: acc.name,
                        extra: acc.message_extra,
                    },
                    finish_reason: acc.finish_reason,
                    extra: acc.choice_extra,
                }
            })
            .collect();

        let mut usage = self.usage;
        if let Some(usage) = usage.as_mut() {
            if usage.total_tokens == 0 {
                usage.total_tokens = usage.prompt_tokens.saturating_add(usage.completion_tokens);
            }
        }

        Ok(ChatCompletionResponse {
            id: self
                .id
                .unwrap_or_else(|| "chatcmpl-openai-compatible-stream".to_string()),
            object: "chat.completion".to_string(),
            created: self
                .created
                .unwrap_or_else(|| chrono::Utc::now().timestamp()),
            model: self
                .model
                .filter(|model| !model.trim().is_empty())
                .unwrap_or(self.requested_model),
            choices,
            usage,
            extra: self.extra,
        })
    }

    fn drain_events(&mut self, flush: bool) -> Result<(), String> {
        while let Some((end, delimiter_len)) = find_event_boundary(&self.pending) {
            let event = self.pending[..end].to_vec();
            self.pending.drain(..end + delimiter_len);
            self.process_event(&event)?;
        }

        if flush && !self.pending.is_empty() {
            let event = std::mem::take(&mut self.pending);
            self.process_event(&event)?;
        }

        Ok(())
    }

    fn process_event(&mut self, event: &[u8]) -> Result<(), String> {
        let text = std::str::from_utf8(event)
            .map_err(|error| format!("upstream SSE event was not valid UTF-8: {error}"))?;

        let mut data_lines = Vec::new();
        for raw_line in text.lines() {
            let line = raw_line.trim_end_matches('\r');
            if line.starts_with(':') {
                continue;
            }
            if let Some(data) = line.strip_prefix("data:") {
                data_lines.push(data.trim_start());
            } else if line == "data" {
                data_lines.push("");
            }
        }

        if data_lines.is_empty() {
            return Ok(());
        }

        let payload = data_lines.join("\n");
        if payload.trim() == "[DONE]" {
            self.saw_done = true;
            return Ok(());
        }

        let value: Value = serde_json::from_str(&payload)
            .map_err(|error| format!("invalid JSON in upstream SSE data event: {error}"))?;
        self.apply_chunk(value)
    }

    fn apply_chunk(&mut self, value: Value) -> Result<(), String> {
        let object = value
            .as_object()
            .ok_or_else(|| "upstream SSE data event must be a JSON object".to_string())?;

        if let Some(id) = object.get("id").and_then(Value::as_str) {
            self.id = Some(id.to_string());
        }
        if let Some(created) = object.get("created").and_then(Value::as_i64) {
            self.created = Some(created);
        }
        if let Some(model) = object.get("model").and_then(Value::as_str) {
            self.model = Some(model.to_string());
        }
        if let Some(usage) = object.get("usage") {
            if !usage.is_null() {
                self.usage = Some(
                    serde_json::from_value(usage.clone())
                        .map_err(|error| format!("invalid stream usage object: {error}"))?,
                );
            }
        }

        for (key, value) in object {
            if !matches!(
                key.as_str(),
                "id" | "object" | "created" | "model" | "choices" | "usage"
            ) {
                self.extra.insert(key.clone(), value.clone());
            }
        }

        let Some(choices) = object.get("choices").and_then(Value::as_array) else {
            return Ok(());
        };

        for choice in choices {
            let choice_object = choice
                .as_object()
                .ok_or_else(|| "stream choice must be a JSON object".to_string())?;

            let index = choice_object
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            let acc = self.choices.entry(index).or_default();

            if let Some(reason) = choice_object.get("finish_reason").and_then(Value::as_str) {
                acc.finish_reason = Some(reason.to_string());
            }

            for (key, value) in choice_object {
                if !matches!(key.as_str(), "index" | "delta" | "finish_reason") {
                    if key == "logprobs" {
                        merge_logprobs(
                            acc.choice_extra.entry(key.clone()).or_insert(Value::Null),
                            value,
                        );
                    } else {
                        merge_json_value(
                            acc.choice_extra.entry(key.clone()).or_insert(Value::Null),
                            value,
                        );
                    }
                }
            }

            let Some(delta) = choice_object.get("delta").and_then(Value::as_object) else {
                continue;
            };

            if let Some(role) = delta.get("role").and_then(Value::as_str) {
                acc.role = Some(role.to_string());
            }
            if let Some(name) = delta.get("name").and_then(Value::as_str) {
                acc.name = Some(name.to_string());
            }
            if let Some(content) = delta.get("content") {
                acc.push_content(content);
            }

            for (key, value) in delta {
                if matches!(key.as_str(), "role" | "name" | "content") {
                    continue;
                }

                if key == "tool_calls" {
                    merge_tool_calls(&mut acc.message_extra, value)?;
                } else if key == "function_call" {
                    merge_function_call(&mut acc.message_extra, value)?;
                } else if matches!(key.as_str(), "refusal" | "reasoning_content" | "reasoning") {
                    append_string_fragment(
                        acc.message_extra.entry(key.clone()).or_insert(Value::Null),
                        value,
                    );
                } else {
                    merge_json_value(
                        acc.message_extra.entry(key.clone()).or_insert(Value::Null),
                        value,
                    );
                }
            }
        }

        Ok(())
    }
}

fn find_event_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    let lf = bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|i| (i, 2));
    let crlf = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|i| (i, 4));

    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn merge_json_value(target: &mut Value, incoming: &Value) {
    match (target, incoming) {
        (Value::Object(target), Value::Object(incoming)) => {
            for (key, value) in incoming {
                merge_json_value(target.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (Value::Array(target), Value::Array(incoming)) => {
            if target.is_empty() {
                *target = incoming.clone();
            } else {
                for (index, value) in incoming.iter().enumerate() {
                    if index >= target.len() {
                        target.push(value.clone());
                    } else {
                        merge_json_value(&mut target[index], value);
                    }
                }
            }
        }
        (target, incoming) => *target = incoming.clone(),
    }
}

fn merge_logprobs(target: &mut Value, incoming: &Value) {
    let Some(incoming_object) = incoming.as_object() else {
        *target = incoming.clone();
        return;
    };
    let Some(target_object) = target.as_object_mut() else {
        *target = Value::Object(incoming_object.clone());
        return;
    };

    for (key, value) in incoming_object {
        if key == "content" {
            match (
                target_object.get_mut(key).and_then(Value::as_array_mut),
                value.as_array(),
            ) {
                (Some(target_content), Some(incoming_content)) => {
                    target_content.extend(incoming_content.clone());
                }
                _ => {
                    target_object.insert(key.clone(), value.clone());
                }
            }
        } else {
            merge_json_value(
                target_object.entry(key.clone()).or_insert(Value::Null),
                value,
            );
        }
    }
}

fn append_string_fragment(target: &mut Value, incoming: &Value) {
    match (target, incoming) {
        (Value::String(target), Value::String(incoming)) => target.push_str(incoming),
        (target, incoming) => *target = incoming.clone(),
    }
}

fn merge_function_call(extra: &mut Map<String, Value>, incoming: &Value) -> Result<(), String> {
    let incoming = incoming
        .as_object()
        .ok_or_else(|| "delta.function_call must be an object".to_string())?;
    let target = extra
        .entry("function_call".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| "internal function_call accumulator is not an object".to_string())?;

    for (key, value) in incoming {
        if key == "arguments" {
            append_string_fragment(target.entry(key.clone()).or_insert(Value::Null), value);
        } else {
            target.insert(key.clone(), value.clone());
        }
    }

    Ok(())
}

fn merge_tool_calls(extra: &mut Map<String, Value>, incoming: &Value) -> Result<(), String> {
    let incoming = incoming
        .as_array()
        .ok_or_else(|| "delta.tool_calls must be an array".to_string())?;

    let entry = extra
        .entry("tool_calls".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let target = entry
        .as_array_mut()
        .ok_or_else(|| "internal tool_calls accumulator is not an array".to_string())?;

    for (position, item) in incoming.iter().enumerate() {
        let object = item
            .as_object()
            .ok_or_else(|| "delta.tool_calls entries must be objects".to_string())?;
        let index = object
            .get("index")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(position);

        while target.len() <= index {
            target.push(Value::Object(Map::new()));
        }

        let target_object = target[index]
            .as_object_mut()
            .ok_or_else(|| "internal tool call accumulator is not an object".to_string())?;

        for (key, value) in object {
            if key == "index" {
                target_object.insert(key.clone(), value.clone());
                continue;
            }

            if key == "function" {
                let function = value
                    .as_object()
                    .ok_or_else(|| "delta.tool_calls.function must be an object".to_string())?;
                let target_function = target_object
                    .entry("function".to_string())
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .ok_or_else(|| "internal function accumulator is not an object".to_string())?;

                for (function_key, function_value) in function {
                    if function_key == "arguments" {
                        if let Some(fragment) = function_value.as_str() {
                            let current = target_function
                                .entry(function_key.clone())
                                .or_insert_with(|| Value::String(String::new()));
                            if let Value::String(current) = current {
                                current.push_str(fragment);
                            } else {
                                *current = Value::String(fragment.to_string());
                            }
                        } else {
                            target_function.insert(function_key.clone(), function_value.clone());
                        }
                    } else {
                        target_function.insert(function_key.clone(), function_value.clone());
                    }
                }
            } else {
                target_object.insert(key.clone(), value.clone());
            }
        }
    }

    Ok(())
}

fn strip_tool_call_indexes(extra: &mut Map<String, Value>) {
    let Some(tool_calls) = extra.get_mut("tool_calls").and_then(Value::as_array_mut) else {
        return;
    };

    for tool_call in tool_calls {
        if let Some(object) = tool_call.as_object_mut() {
            object.remove("index");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconstructs_content_usage_and_tool_call_fragments() {
        let mut assembler = OpenAiStreamAssembler::new("requested-model", 1024 * 1024);
        assembler
            .push_bytes(
                br#"data: {"id":"abc","created":123,"model":"m","choices":[{"index":0,"delta":{"role":"assistant","content":"Hello ","tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"loc"}}]},"finish_reason":null}]}

data: {"id":"abc","created":123,"model":"m","choices":[{"index":0,"delta":{"content":"world","tool_calls":[{"index":0,"function":{"arguments":"ation\":\"Paris\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}

data: [DONE]

"#,
            )
            .unwrap();

        let response = assembler.finish().unwrap();
        assert_eq!(response.id, "abc");
        assert_eq!(response.model, "m");
        assert_eq!(
            response.choices[0].message.content,
            Value::String("Hello world".into())
        );
        assert_eq!(
            response.choices[0].message.extra["tool_calls"][0]["function"]["arguments"],
            Value::String("{\"location\":\"Paris\"}".into())
        );
        assert_eq!(response.usage.unwrap().total_tokens, 15);
    }

    #[test]
    fn reconstructs_legacy_function_call_and_reasoning_fragments() {
        let mut assembler = OpenAiStreamAssembler::new("m", 1024 * 1024);
        assembler
            .push_bytes(
                br#"data: {"choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"think ","function_call":{"name":"lookup","arguments":"{\"q\":\""}},"finish_reason":null}]}

data: {"choices":[{"index":0,"delta":{"reasoning_content":"again","function_call":{"arguments":"value\"}"}},"finish_reason":"function_call"}]}

data: [DONE]

"#,
            )
            .unwrap();

        let response = assembler.finish().unwrap();
        let message = &response.choices[0].message;
        assert_eq!(
            message.extra["reasoning_content"],
            Value::String("think again".into())
        );
        assert_eq!(
            message.extra["function_call"]["arguments"],
            Value::String("{\"q\":\"value\"}".into())
        );
    }

    #[test]
    fn rejects_truncated_stream_without_terminal_marker() {
        let mut assembler = OpenAiStreamAssembler::new("m", 1024);
        assembler
            .push_bytes(
                br#"data: {"choices":[{"index":0,"delta":{"content":"partial"},"finish_reason":null}]}

"#,
            )
            .unwrap();

        let error = assembler.finish().unwrap_err();
        assert!(error.contains("ended before"));
    }

    #[test]
    fn enforces_buffer_limit() {
        let mut assembler = OpenAiStreamAssembler::new("m", 4);
        let error = assembler.push_bytes(b"12345").unwrap_err();
        assert!(error.contains("maximum buffered size"));
    }
}
