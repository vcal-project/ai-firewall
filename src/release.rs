pub const PRODUCT_NAME: &str = "AI Cost Firewall";

pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const RELEASE_TITLE: &str = "Production Hardening I";

pub const SUPPORTED_API_STYLE: &str = "openai_compatible";

pub const COMPATIBILITY_MODEL: &str =
    "OpenAI-compatible chat and embedding APIs through a simple flat configuration model";

pub const SCOPE_NOTE: &str =
    "v0.5.0 hardens production behavior with fallible guard initialization, safer fail-closed guard defaults, configuration/readiness invariants, explicit operational hardening warnings, bounded VCAL Audit retry backoff with Retry-After support, stable guard error contracts, and non-panicking shutdown signal setup";

pub const API_COMPATIBILITY_VERSION: &str = "v1";
pub const CONFIG_SCHEMA_VERSION: u32 = 1;
