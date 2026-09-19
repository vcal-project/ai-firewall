pub const PRODUCT_NAME: &str = "AI Cost Firewall";

pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const RELEASE_TITLE: &str = "Controlled Streaming";

pub const SUPPORTED_API_STYLE: &str = "openai_compatible";

pub const COMPATIBILITY_MODEL: &str =
    "OpenAI-compatible chat and embedding APIs through a simple flat configuration model";

pub const SCOPE_NOTE: &str =
    "v0.7.0 adds controlled OpenAI-compatible streaming: upstream SSE is assembled into a canonical completion, processed through response controls, privacy restoration, accounting, cache storage, and evidence before any model-generated content is committed to the client.";

pub const API_COMPATIBILITY_VERSION: &str = "v1";
pub const CONFIG_SCHEMA_VERSION: u32 = 1;
