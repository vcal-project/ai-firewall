use crate::types::openai::ChatCompletionResponse;

/// A provider may finish a valid HTTP/SSE response because it ran out of tokens.
/// Such a response must still be returned to the caller, but is not reusable.
pub fn cacheable_response(response: &ChatCompletionResponse) -> bool {
    !response.choices.is_empty()
        && response.choices.iter().all(|choice| {
            let content = choice
                .message
                .content
                .as_str()
                .is_some_and(|text| !text.trim().is_empty());
            let tool_call = choice
                .message
                .extra
                .get("tool_calls")
                .and_then(|value| value.as_array())
                .is_some_and(|calls| !calls.is_empty());
            let function_call = choice
                .message
                .extra
                .get("function_call")
                .is_some_and(|value| !value.is_null());
            match choice.finish_reason.as_deref() {
                Some("stop") => content,
                Some("tool_calls") => tool_call,
                Some("function_call") => function_call,
                _ => false,
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::openai::{ChatMessage, Choice};
    use serde_json::{json, Map};

    fn response(reason: &str, content: serde_json::Value) -> ChatCompletionResponse {
        ChatCompletionResponse {
            id: "test".into(),
            object: "chat.completion".into(),
            created: 1,
            model: "m".into(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".into(),
                    content,
                    name: None,
                    extra: Map::new(),
                },
                finish_reason: Some(reason.into()),
                extra: Map::new(),
            }],
            usage: None,
            extra: Map::new(),
        }
    }

    #[test]
    fn rejects_truncated_and_empty_visible_answers() {
        assert!(!cacheable_response(&response("length", json!("partial"))));
        assert!(!cacheable_response(&response("stop", json!(""))));
        assert!(!cacheable_response(&response("stop", json!(null))));
        assert!(cacheable_response(&response("stop", json!("complete"))));
    }

    #[test]
    fn accepts_valid_tool_call_completion() {
        let mut result = response("tool_calls", json!(null));
        result.choices[0].message.extra.insert(
            "tool_calls".into(),
            json!([{
                "id":"call_1", "type":"function", "function": {"name":"lookup","arguments":"{}"}
            }]),
        );
        assert!(cacheable_response(&result));
    }
}
