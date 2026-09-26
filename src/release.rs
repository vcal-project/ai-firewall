pub const PRODUCT_NAME: &str = "AI Cost Firewall";

pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const RELEASE_TITLE: &str = "Evaluation Mode";

pub const SUPPORTED_API_STYLE: &str = "openai_compatible";

pub const COMPATIBILITY_MODEL: &str =
    "OpenAI-compatible chat and embedding APIs through a simple flat configuration model";

pub const SCOPE_NOTE: &str =
    "v0.8.0 adds AIF Evaluation Mode: observe-mode requests use isolated exact and semantic shadow cache state, record hypothetical cache actions and savings, and always return the live upstream response without allowing evaluation cache failures to interrupt application traffic.";

pub const API_COMPATIBILITY_VERSION: &str = "v1";
pub const CONFIG_SCHEMA_VERSION: u32 = 1;
