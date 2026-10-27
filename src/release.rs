pub const PRODUCT_NAME: &str = "AI Cost Firewall";

pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const RELEASE_TITLE: &str = "Security Maintenance and Custom CA Trust";

pub const SUPPORTED_API_STYLE: &str = "openai_compatible";

pub const COMPATIBILITY_MODEL: &str =
    "OpenAI-compatible chat, model discovery, and embedding APIs through a simple flat configuration model";

pub const SCOPE_NOTE: &str =
    "v0.8.7 adds optional PEM CA bundles for private HTTPS upstream LLM and embedding providers via upstream_ca_bundle and embedding_ca_bundle. Custom roots are added to the existing trusted roots for both buffered and controlled-streaming chat clients and the embedding client, while TLS certificate and hostname verification remain enabled. Missing, invalid, or unreadable configured bundles fail initialization; CA certificate rotation requires an AIF restart. The release adds loopback TLS regression tests and OpenShift service CA deployment guidance, and updates Tokio to address RUSTSEC-2026-0386. Existing Observe/Enforce behavior, caching, Guard integrations, and Assessment Context remain supported. The API and configuration schema versions are unchanged.";

pub const API_COMPATIBILITY_VERSION: &str = "v1";
pub const CONFIG_SCHEMA_VERSION: u32 = 1;
pub const ASSESSMENT_CONTEXT_SCHEMA_VERSION: &str = "1.0";
