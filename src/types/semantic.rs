use serde::{Deserialize, Serialize};

use crate::types::openai::ChatCompletionResponse;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticCacheRecord {
    pub request_hash: String,
    pub model: String,
    pub normalized_prompt: String,
    pub response: ChatCompletionResponse,
    pub privacy_cache_scope: Option<String>,
    pub privacy_placeholder_signature: String,
    pub inserted_at: i64,
    pub expires_at: i64,
}
