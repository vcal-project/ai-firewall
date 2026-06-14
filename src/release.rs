pub const PRODUCT_NAME: &str = "AI Cost Firewall";

pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const RELEASE_TITLE: &str = "VCAL Privacy Guard Orchestration Preview";

pub const SUPPORTED_API_STYLE: &str = "openai_compatible";

pub const COMPATIBILITY_MODEL: &str =
    "OpenAI-compatible chat and embedding APIs through a simple flat configuration model";

pub const SCOPE_NOTE: &str =
    "v0.2.2 introduces optional VCAL Privacy Guard orchestration with pre-upstream anonymization, post-upstream restoration, configurable guard fail-open or fail-closed behavior, API-key protected guard calls, stream rejection when privacy restoration is enabled, and safer handling of non-string message content";
