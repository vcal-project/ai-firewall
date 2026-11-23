pub const PRODUCT_NAME: &str = "AI Cost Firewall";

pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const RELEASE_TITLE: &str = "Progressive Streaming and Cache-Aware SSE Delivery";

pub const SUPPORTED_API_STYLE: &str = "openai_compatible";

pub const COMPATIBILITY_MODEL: &str =
    "OpenAI-compatible chat, model discovery, and embedding APIs through a simple flat configuration model";

pub const SCOPE_NOTE: &str =
    "v0.8.9 adds cache-aware progressive upstream Server-Sent Events (SSE) delivery for eligible streaming chat requests. The streaming_delivery_mode setting supports controlled, auto, and progressive selection. Eligible upstream cache misses can forward validated SSE content incrementally, while completed exact and semantic cache hits retain immediate SSE replay without artificial generation delays. Controlled delivery remains available and is selected when full-response Security Guard inspection, Privacy Guard restoration, or other safety constraints require buffering. Progressive generation reconstructs the completed response for cache storage and accounting, and distinguishes successful completion from upstream stream failures and client disconnections; incomplete streams must not be cached as successful responses. Updated startup diagnostics distinguish requested, selected, and request-dependent active delivery, and low-cardinality Prometheus counters expose selected streaming paths. Existing Observe/Enforce cache semantics, Redis AOF persistence, configured fail-open/fail-closed behavior, Guard enforcement, and TLS verification remain in effect. OpenAI-compatible API and configuration schema versions remain unchanged.";

pub const API_COMPATIBILITY_VERSION: &str = "v1";
pub const CONFIG_SCHEMA_VERSION: u32 = 1;
pub const ASSESSMENT_CONTEXT_SCHEMA_VERSION: &str = "1.0";
