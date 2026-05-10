use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiCompatEndpoint {
    ChatCompletions,
    Embeddings,
}

impl OpenAiCompatEndpoint {
    fn path(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat/completions",
            Self::Embeddings => "embeddings",
        }
    }

    fn forbidden_suffixes() -> &'static [&'static str] {
        &["/chat/completions", "/embeddings"]
    }
}

pub fn build_openai_compat_url(
    base_url: &str,
    endpoint: OpenAiCompatEndpoint,
) -> Result<String, AppError> {
    let trimmed = base_url.trim().trim_end_matches('/');

    if trimmed.is_empty() {
        return Err(AppError::internal(
            "OpenAI-compatible base URL must not be empty",
        ));
    }

    let lower = trimmed.to_ascii_lowercase();

    for suffix in OpenAiCompatEndpoint::forbidden_suffixes() {
        if lower.ends_with(suffix) {
            return Err(AppError::bad_request(format!(
                "OpenAI-compatible base URL must be a base URL, not a full endpoint path: '{}'. Use the provider root URL or its /v1 base path.",
                base_url
            )));
        }
    }

    if lower.ends_with("/v1") {
        Ok(format!("{}/{}", trimmed, endpoint.path()))
    } else {
        Ok(format!("{}/v1/{}", trimmed, endpoint.path()))
    }
}

pub fn should_send_bearer_auth(api_key: &str) -> bool {
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
    fn builds_chat_url_from_root_base_url() {
        let url = build_openai_compat_url(
            "https://api.openai.com",
            OpenAiCompatEndpoint::ChatCompletions,
        )
        .unwrap();

        assert_eq!(url, "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn builds_chat_url_from_v1_base_url() {
        let url = build_openai_compat_url(
            "http://ollama:11434/v1",
            OpenAiCompatEndpoint::ChatCompletions,
        )
        .unwrap();

        assert_eq!(url, "http://ollama:11434/v1/chat/completions");
    }

    #[test]
    fn builds_embeddings_url_from_root_base_url() {
        let url = build_openai_compat_url("http://ollama:11434", OpenAiCompatEndpoint::Embeddings)
            .unwrap();

        assert_eq!(url, "http://ollama:11434/v1/embeddings");
    }

    #[test]
    fn builds_embeddings_url_from_v1_base_url() {
        let url =
            build_openai_compat_url("http://ollama:11434/v1", OpenAiCompatEndpoint::Embeddings)
                .unwrap();

        assert_eq!(url, "http://ollama:11434/v1/embeddings");
    }

    #[test]
    fn rejects_full_chat_endpoint_as_base_url() {
        let err = build_openai_compat_url(
            "http://ollama:11434/v1/chat/completions",
            OpenAiCompatEndpoint::ChatCompletions,
        )
        .unwrap_err();

        assert!(err.to_string().contains("not a full endpoint path"));
    }

    #[test]
    fn rejects_full_embeddings_endpoint_as_base_url() {
        let err = build_openai_compat_url(
            "http://ollama:11434/v1/embeddings",
            OpenAiCompatEndpoint::Embeddings,
        )
        .unwrap_err();

        assert!(err.to_string().contains("not a full endpoint path"));
    }

    #[test]
    fn placeholder_keys_do_not_send_auth() {
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
