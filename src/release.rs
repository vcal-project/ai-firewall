pub const PRODUCT_NAME: &str = "AI Cost Firewall";

pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const RELEASE_TITLE: &str = "Deployment Hardening";

pub const SUPPORTED_API_STYLE: &str = "openai_compatible";

pub const COMPATIBILITY_MODEL: &str =
    "OpenAI-compatible chat, model discovery, and embedding APIs through a simple flat configuration model";

pub const SCOPE_NOTE: &str =
    "v0.8.2 hardens the v0.8 Evaluation Mode baseline for container and orchestrator deployment: exact-cache identity preserves OpenAI-compatible extension fields, non-string message content bypasses semantic cache, strict startup probing detects required cache backends that failed to initialize, /v1/models is proxied to the configured chat upstream, and OpenShift restricted-v2 manifests are provided as a deployment-specific option.";

pub const API_COMPATIBILITY_VERSION: &str = "v1";
pub const CONFIG_SCHEMA_VERSION: u32 = 1;
