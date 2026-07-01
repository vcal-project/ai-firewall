pub const PRODUCT_NAME: &str = "AI Cost Firewall";

pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const RELEASE_TITLE: &str = "Guard Orchestration and Security Guard Integration";

pub const SUPPORTED_API_STYLE: &str = "openai_compatible";

pub const COMPATIBILITY_MODEL: &str =
    "OpenAI-compatible chat and embedding APIs through a simple flat configuration model";

pub const SCOPE_NOTE: &str =
    "v0.3.0 adds modular VCAL guard orchestration with optional Privacy Guard and Security Guard stages, raw request security scanning before privacy anonymization, response security scanning before privacy restoration, guard-specific error handling, AI Firewall-level guard metrics, Security Guard configuration support, and preservation of unknown OpenAI-compatible request and response fields";
