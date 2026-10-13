pub const PRODUCT_NAME: &str = "AI Cost Firewall";

pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const RELEASE_TITLE: &str = "Security Guard Structured Content Compatibility";

pub const SUPPORTED_API_STYLE: &str = "openai_compatible";

pub const COMPATIBILITY_MODEL: &str =
    "OpenAI-compatible chat, model discovery, and embedding APIs through a simple flat configuration model";

pub const SCOPE_NOTE: &str =
    "v0.8.5 fixes AIF compatibility with VCAL Security Guard v0.3.0 for structured message content: request- and response-side scans preserve the original JSON content, including text-part arrays, instead of silently omitting non-string messages. Security inspection now covers supported structured text before cache lookup and upstream forwarding, while unsupported shapes are passed to Security Guard for validation. Existing Privacy Guard v0.3.0 hardening, Evaluation Mode, controlled streaming, and Assessment Context remain supported. The API and configuration schema versions are unchanged.";

pub const API_COMPATIBILITY_VERSION: &str = "v1";
pub const CONFIG_SCHEMA_VERSION: u32 = 1;
pub const ASSESSMENT_CONTEXT_SCHEMA_VERSION: &str = "1.0";
