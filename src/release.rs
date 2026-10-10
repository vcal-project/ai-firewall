pub const PRODUCT_NAME: &str = "AI Cost Firewall";

pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const RELEASE_TITLE: &str = "Privacy Guard v0.3 Hardening";

pub const SUPPORTED_API_STYLE: &str = "openai_compatible";

pub const COMPATIBILITY_MODEL: &str =
    "OpenAI-compatible chat, model discovery, and embedding APIs through a simple flat configuration model";

pub const SCOPE_NOTE: &str =
    "v0.8.4 hardens the AIF integration with VCAL Privacy Guard v0.3.0: structured text content is inspected without flattening request shape, unsupported text-bearing shapes fail closed, effective privacy policy and inspection coverage metadata are preserved in evidence, exact and semantic caches are isolated by an opaque tenant/policy scope, fail-open privacy outages bypass caches, and response restoration remains request-local so cached responses retain placeholders rather than restored personal information. Evaluation Mode, controlled streaming, and the v0.8.3 Assessment Context remain supported.";

pub const API_COMPATIBILITY_VERSION: &str = "v1";
pub const CONFIG_SCHEMA_VERSION: u32 = 1;
pub const ASSESSMENT_CONTEXT_SCHEMA_VERSION: &str = "1.0";
