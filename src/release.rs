pub const PRODUCT_NAME: &str = "AI Cost Firewall";

pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const RELEASE_TITLE: &str = "Redis Recovery and Operational Resilience";

pub const SUPPORTED_API_STYLE: &str = "openai_compatible";

pub const COMPATIBILITY_MODEL: &str =
    "OpenAI-compatible chat, model discovery, and embedding APIs through a simple flat configuration model";

pub const SCOPE_NOTE: &str =
    "v0.8.8 improves resilience when Redis exact caching becomes unavailable or rejects writes, including AOF-related MISCONF errors. A periodic SETEX write-health probe restores dependency readiness and exact-cache operation after Redis recovers without restarting AIF, even when no requests arrive. Fail-open or Observe startup retains a reconnectable Redis client; configured Enforce fail-closed startup remains unchanged. Runtime reload keeps readiness attached to the active dependency state, and --readycheck provides dependency-aware CLI readiness while --healthcheck remains liveness-only. New low-cardinality Prometheus metrics distinguish Redis write health, probe failures, reconnect attempts, and health transitions. Redis AOF persistence, configured fail-open/fail-closed policies, Observe/Enforce modes, Qdrant semantic caching, controlled SSE response delivery, Guard integrations, and TLS verification remain unchanged. Progressive upstream token streaming is not introduced. API and configuration schema versions remain unchanged.";

pub const API_COMPATIBILITY_VERSION: &str = "v1";
pub const CONFIG_SCHEMA_VERSION: u32 = 1;
pub const ASSESSMENT_CONTEXT_SCHEMA_VERSION: &str = "1.0";
