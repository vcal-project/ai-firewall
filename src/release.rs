pub const PRODUCT_NAME: &str = "AI Cost Firewall";

pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const RELEASE_TITLE: &str = "Assessment Context";

pub const SUPPORTED_API_STYLE: &str = "openai_compatible";

pub const COMPATIBILITY_MODEL: &str =
    "OpenAI-compatible chat, model discovery, and embedding APIs through a simple flat configuration model";

pub const SCOPE_NOTE: &str =
    "v0.8.3 builds on the v0.8.2 Deployment Hardening baseline with assessment integration for VCAL Console v0.3 and future consumers: /assessment-context exposes an allow-listed, non-secret snapshot of effective AIF configuration and pricing assumptions, a stable configuration fingerprint supports reproducible assessments, /version advertises the assessment-context schema, aif_runtime_info records version/configuration identity for historical stability checks, and SIGHUP reload rejects settings that require a process restart rather than reporting them as effective.";

pub const API_COMPATIBILITY_VERSION: &str = "v1";
pub const CONFIG_SCHEMA_VERSION: u32 = 1;
pub const ASSESSMENT_CONTEXT_SCHEMA_VERSION: &str = "1.0";
