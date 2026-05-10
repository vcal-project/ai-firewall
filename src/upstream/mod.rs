pub mod llm;
pub mod openai;
pub mod openai_compat;

use std::sync::Arc;
use std::time::Duration;

use crate::config::{Config, ProviderKind};
use crate::error::AppError;

use self::llm::LlmUpstream;
use self::openai::OpenAiUpstream;

pub fn build_llm_upstream(cfg: &Config) -> Result<Arc<dyn LlmUpstream>, AppError> {
    match cfg.upstream_provider {
        ProviderKind::OpenAiCompatible => Ok(Arc::new(OpenAiUpstream::new(
            cfg.upstream_base_url.clone(),
            cfg.upstream_api_key.clone(),
            Duration::from_secs(cfg.request_timeout_seconds),
        )?)),
    }
}
