use serde::{Deserialize, Serialize};

use crate::types::openai::ChatCompletionResponse;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticCacheRecord {
    /// Stable hash of the normalized prompt text.
    pub request_hash: String,
    /// Model name the cached response belongs to.
    pub model: String,
    /// Normalized prompt text used for embedding and lookup.
    pub normalized_prompt: String,
    /// Cached upstream chat-completion response.
    pub response: ChatCompletionResponse,
    /// Unix timestamp when the record was inserted.
    pub inserted_at: i64,
    /// Unix timestamp after which the record is considered expired.
    pub expires_at: i64,
}
