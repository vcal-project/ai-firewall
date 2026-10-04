//! Redacted, assessment-oriented view of the effective AIF configuration.
//!
//! This module is intentionally allow-list based. It must never return the full
//! `Config` structure or expose credentials, secret-bearing URLs, tenant identifiers,
//! or other deployment details merely because they exist in `Config`.

use crate::{config::Config, evidence::EVIDENCE_SCHEMA_VERSION, release};
use reqwest::Url;
use serde::Serialize;
use sha2::{Digest, Sha256};

const CONFIGURATION_HASH_DOMAIN: &[u8] = b"aif-assessment-context:v1\0";

#[derive(Serialize)]
struct AssessmentHashInput<'a> {
    schema_version: &'static str,
    configuration: &'a AssessmentConfigurationV1,
    routing: RoutingIdentity,
}

#[derive(Serialize)]
struct RoutingIdentity {
    upstream: String,
    exact_cache_backend: Option<String>,
    semantic_cache_backend: Option<String>,
    embedding_backend: Option<String>,
    security_guard: Option<String>,
    privacy_guard: Option<String>,
    privacy_tenant: Option<String>,
    usage_guard: Option<String>,
    usage_tenant: Option<String>,
    audit: Option<String>,
    audit_producer_instance_id: Option<String>,
}

fn endpoint_identity(raw: &str) -> String {
    let Ok(mut url) = Url::parse(raw) else {
        return raw.trim().to_string();
    };

    // Endpoint identity is hashed but never returned. Strip passwords and
    // query/fragment data so secret rotation does not alter the fingerprint.
    // Keep the username because changing an ACL identity can change behavior.
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

// Routing identity is included only in the hash input. It lets Console detect
// backend/service changes without placing internal service URLs in the response.
fn routing_identity(cfg: &Config) -> RoutingIdentity {
    RoutingIdentity {
        upstream: endpoint_identity(&cfg.upstream_base_url),
        exact_cache_backend: cfg
            .exact_cache_enabled
            .then(|| endpoint_identity(&cfg.redis_url)),
        semantic_cache_backend: cfg
            .semantic_cache_enabled
            .then(|| endpoint_identity(&cfg.qdrant_url)),
        embedding_backend: cfg
            .semantic_cache_enabled
            .then(|| endpoint_identity(&cfg.embedding_base_url)),
        security_guard: cfg
            .security_guard_enabled
            .then(|| endpoint_identity(&cfg.security_guard_url)),
        privacy_guard: cfg
            .privacy_guard_enabled
            .then(|| endpoint_identity(&cfg.privacy_guard_url)),
        privacy_tenant: cfg
            .privacy_guard_enabled
            .then(|| cfg.privacy_guard_tenant_id.clone())
            .flatten(),
        usage_guard: cfg
            .usage_guard_enabled
            .then(|| endpoint_identity(&cfg.usage_guard_url)),
        usage_tenant: cfg
            .usage_guard_enabled
            .then(|| cfg.usage_guard_tenant_id.clone())
            .flatten(),
        audit: cfg.audit_enabled.then(|| endpoint_identity(&cfg.audit_url)),
        audit_producer_instance_id: cfg
            .audit_enabled
            .then(|| Some(cfg.audit_producer_instance_id.clone()))
            .flatten(),
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct AssessmentContextV1 {
    pub schema_version: &'static str,
    pub product: ProductContext,
    pub configuration: AssessmentConfigurationV1,
    pub configuration_hash: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ProductContext {
    pub name: &'static str,
    pub version: &'static str,
    pub api_compatibility: &'static str,
    pub config_schema: u32,
    pub evidence_schema: &'static str,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct AssessmentConfigurationV1 {
    pub mode: ModeContext,
    pub exact_cache: ExactCacheContext,
    pub semantic_cache: SemanticCacheContext,
    pub embedding: EmbeddingContext,
    pub upstream: UpstreamContext,
    pub request_path: RequestPathContext,
    pub pricing: PricingContext,
    pub optional_modules: OptionalModulesContext,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ModeContext {
    pub enforcement_mode: &'static str,
    pub effective_cache_scope: &'static str,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ExactCacheContext {
    pub enabled: bool,
    pub store_enabled: bool,
    pub fail_open: bool,
    pub ttl_seconds: usize,
    pub operation_timeout_seconds: u64,
    pub effective_prefix: &'static str,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct SemanticCacheContext {
    pub enabled: bool,
    pub store_enabled: bool,
    pub fail_open: bool,
    pub similarity_threshold: f32,
    pub retention_seconds: usize,
    pub configured_collection: String,
    pub effective_collection: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct EmbeddingContext {
    pub provider: &'static str,
    pub model: String,
    pub vector_size: u64,
    pub timeout_seconds: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct UpstreamContext {
    pub provider: &'static str,
    pub timeout_seconds: u64,
    pub allow_unknown_models_pass_through: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct RequestPathContext {
    pub controlled_streaming_enabled: bool,
    pub max_stream_upstream_bytes: usize,
    pub max_request_body_bytes: usize,
    pub max_prompt_chars: usize,
    pub max_inflight_requests: usize,
    pub max_inflight_upstream_requests: usize,
    pub cache_bypass_header: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PricingContext {
    pub models: Vec<ModelPriceContext>,
    pub embedding_usd_per_1m_tokens: Option<f64>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ModelPriceContext {
    pub model: String,
    pub input_usd_per_1m_tokens: f64,
    pub output_usd_per_1m_tokens: f64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct OptionalModulesContext {
    pub guard_fail_open: bool,
    pub security_guard: SecurityGuardContext,
    pub privacy_guard: PrivacyGuardContext,
    pub usage_guard: UsageGuardContext,
    pub audit: AuditContext,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct SecurityGuardContext {
    pub enabled: bool,
    pub timeout_seconds: u64,
    pub block_response: &'static str,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PrivacyGuardContext {
    pub enabled: bool,
    pub mode: &'static str,
    pub restore_enabled: bool,
    pub timeout_seconds: u64,
    pub policy_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct UsageGuardContext {
    pub enabled: bool,
    pub mode: &'static str,
    pub timeout_seconds: u64,
    pub policy_id: Option<String>,
    pub block_response: &'static str,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct AuditContext {
    pub enabled: bool,
}

impl AssessmentConfigurationV1 {
    pub fn from_config(cfg: &Config) -> Self {
        let mut models: Vec<ModelPriceContext> = cfg
            .model_prices
            .iter()
            .map(|(model, price)| ModelPriceContext {
                model: model.clone(),
                input_usd_per_1m_tokens: price.input_usd_per_1m_tokens,
                output_usd_per_1m_tokens: price.output_usd_per_1m_tokens,
            })
            .collect();
        models.sort_by(|a, b| a.model.cmp(&b.model));

        Self {
            mode: ModeContext {
                enforcement_mode: cfg.aif_enforcement_mode.as_str(),
                effective_cache_scope: if cfg.aif_enforcement_mode.is_observe() {
                    "evaluation"
                } else {
                    "production"
                },
            },
            exact_cache: ExactCacheContext {
                enabled: cfg.exact_cache_enabled,
                store_enabled: cfg.exact_cache_store_enabled,
                fail_open: cfg.exact_cache_fail_open,
                ttl_seconds: cfg.exact_cache_ttl_seconds,
                operation_timeout_seconds: cfg.redis_timeout_seconds,
                effective_prefix: cfg.effective_exact_cache_prefix(),
            },
            semantic_cache: SemanticCacheContext {
                enabled: cfg.semantic_cache_enabled,
                store_enabled: cfg.semantic_cache_store_enabled,
                fail_open: cfg.semantic_cache_fail_open,
                similarity_threshold: cfg.semantic_similarity_threshold,
                retention_seconds: cfg.semantic_cache_retention_seconds,
                configured_collection: cfg.qdrant_collection.clone(),
                effective_collection: cfg.effective_qdrant_collection(),
            },
            embedding: EmbeddingContext {
                provider: cfg.embedding_provider.as_str(),
                model: cfg.embedding_model.clone(),
                vector_size: cfg.qdrant_vector_size,
                timeout_seconds: cfg.embedding_timeout_seconds,
            },
            upstream: UpstreamContext {
                provider: cfg.upstream_provider.as_str(),
                timeout_seconds: cfg.upstream_timeout_seconds,
                allow_unknown_models_pass_through: cfg.allow_unknown_models_pass_through,
            },
            request_path: RequestPathContext {
                controlled_streaming_enabled: cfg.streaming_enabled,
                max_stream_upstream_bytes: cfg.max_stream_upstream_bytes,
                max_request_body_bytes: cfg.max_request_body_bytes,
                max_prompt_chars: cfg.max_prompt_chars,
                max_inflight_requests: cfg.max_inflight_requests,
                max_inflight_upstream_requests: cfg.max_inflight_upstream_requests,
                cache_bypass_header: cfg.cache_bypass_header.clone(),
            },
            pricing: PricingContext {
                models,
                embedding_usd_per_1m_tokens: cfg
                    .embedding_price
                    .as_ref()
                    .map(|price| price.usd_per_1m_tokens),
            },
            optional_modules: OptionalModulesContext {
                guard_fail_open: cfg.guard_fail_open,
                security_guard: SecurityGuardContext {
                    enabled: cfg.security_guard_enabled,
                    timeout_seconds: cfg.security_guard_timeout_seconds,
                    block_response: cfg.security_guard_block_response.as_str(),
                },
                privacy_guard: PrivacyGuardContext {
                    enabled: cfg.privacy_guard_enabled,
                    mode: cfg.privacy_guard_mode.as_str(),
                    restore_enabled: cfg.privacy_guard_restore_enabled,
                    timeout_seconds: cfg.privacy_guard_timeout_seconds,
                    policy_id: cfg.privacy_guard_policy_id.clone(),
                },
                usage_guard: UsageGuardContext {
                    enabled: cfg.usage_guard_enabled,
                    mode: cfg.usage_guard_mode.as_str(),
                    timeout_seconds: cfg.usage_guard_timeout_seconds,
                    policy_id: cfg.usage_guard_policy_id.clone(),
                    block_response: cfg.usage_guard_block_response.as_str(),
                },
                audit: AuditContext {
                    enabled: cfg.audit_enabled,
                },
            },
        }
    }
}

fn configuration_hash(
    cfg: &Config,
    configuration: &AssessmentConfigurationV1,
) -> Result<String, serde_json::Error> {
    let hash_input = AssessmentHashInput {
        schema_version: release::ASSESSMENT_CONTEXT_SCHEMA_VERSION,
        configuration,
        routing: routing_identity(cfg),
    };
    let canonical = serde_json::to_vec(&hash_input)?;
    let mut hasher = Sha256::new();
    hasher.update(CONFIGURATION_HASH_DOMAIN);
    hasher.update(canonical);
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

impl AssessmentContextV1 {
    /// Build the stable v1 assessment contract from the configuration currently
    /// applied by AIF. The returned hash covers both the visible allow-listed
    /// configuration and redacted routing identity.
    pub fn from_config(cfg: &Config) -> Result<Self, serde_json::Error> {
        let configuration = AssessmentConfigurationV1::from_config(cfg);
        let configuration_hash = configuration_hash(cfg, &configuration)?;

        Ok(Self {
            schema_version: release::ASSESSMENT_CONTEXT_SCHEMA_VERSION,
            product: ProductContext {
                name: release::PRODUCT_NAME,
                version: release::PRODUCT_VERSION,
                api_compatibility: release::API_COMPATIBILITY_VERSION,
                config_schema: release::CONFIG_SCHEMA_VERSION,
                evidence_schema: EVIDENCE_SCHEMA_VERSION,
            },
            configuration,
            configuration_hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_config_path(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{name}_{nanos}.conf"))
    }

    fn load_config(extra: &str) -> Config {
        let path = temp_config_path("aif_assessment_context");
        let text = format!(
            r#"
listen_addr 127.0.0.1:8080;
redis_url redis://user:redis-secret@127.0.0.1:6379;
upstream_base_url http://internal-chat.example:8000;
upstream_api_key upstream-secret;
embedding_base_url http://internal-embedding.example:8001;
embedding_api_key embedding-secret;
embedding_model nomic-embed-text;
embedding_price 0.020;
qdrant_url http://internal-qdrant.example:6334;
qdrant_api_key qdrant-secret;
qdrant_collection aif_semantic_cache;
qdrant_vector_size 768;
semantic_cache_enabled true;
semantic_similarity_threshold 0.92;
security_guard_api_key security-secret;
privacy_guard_api_key privacy-secret;
usage_guard_api_key usage-secret;
audit_api_key audit-secret;
metrics_auth_token metrics-secret;
model_price model-b 0.20 0.40;
model_price model-a 0.10 0.30;
{extra}
"#
        );
        fs::write(&path, text).expect("test config should be writable");
        let cfg = Config::from_file(&path).expect("test config should parse");
        fs::remove_file(path).ok();
        cfg
    }

    #[test]
    fn context_contains_effective_observe_scope_and_sorted_prices() {
        let cfg = load_config("aif_enforcement_mode observe;");
        let context = AssessmentContextV1::from_config(&cfg).expect("context should serialize");

        assert_eq!(context.schema_version, "1.0");
        assert_eq!(context.configuration.mode.enforcement_mode, "observe");
        assert_eq!(
            context.configuration.mode.effective_cache_scope,
            "evaluation"
        );
        assert_eq!(
            context.configuration.exact_cache.effective_prefix,
            "chatcmpl:eval:v1"
        );
        assert_eq!(
            context.configuration.semantic_cache.effective_collection,
            "aif_semantic_cache_eval"
        );
        assert_eq!(context.configuration.embedding.model, "nomic-embed-text");
        assert_eq!(context.configuration.embedding.vector_size, 768);
        assert_eq!(context.configuration.pricing.models[0].model, "model-a");
        assert_eq!(context.configuration.pricing.models[1].model, "model-b");
    }

    #[test]
    fn context_serialization_omits_secrets_urls_and_tenant_ids() {
        let cfg = load_config(
            "privacy_guard_tenant_id tenant-secret;\nusage_guard_tenant_id usage-tenant-secret;",
        );
        let context = AssessmentContextV1::from_config(&cfg).expect("context should serialize");
        let json = serde_json::to_string(&context).expect("context JSON should serialize");

        for forbidden in [
            "upstream-secret",
            "embedding-secret",
            "qdrant-secret",
            "security-secret",
            "privacy-secret",
            "usage-secret",
            "audit-secret",
            "metrics-secret",
            "redis-secret",
            "internal-chat.example",
            "internal-embedding.example",
            "internal-qdrant.example",
            "tenant-secret",
            "usage-tenant-secret",
        ] {
            assert!(!json.contains(forbidden), "context leaked {forbidden}");
        }
    }

    #[test]
    fn hash_is_stable_across_price_declaration_order() {
        let first = load_config("");
        let path = temp_config_path("aif_assessment_context_prices");
        let text = r#"
listen_addr 127.0.0.1:8080;
redis_url redis://user:redis-secret@127.0.0.1:6379;
upstream_base_url http://internal-chat.example:8000;
upstream_api_key upstream-secret;
embedding_base_url http://internal-embedding.example:8001;
embedding_api_key embedding-secret;
embedding_model nomic-embed-text;
embedding_price 0.020;
qdrant_url http://internal-qdrant.example:6334;
qdrant_api_key qdrant-secret;
qdrant_collection aif_semantic_cache;
qdrant_vector_size 768;
semantic_cache_enabled true;
semantic_similarity_threshold 0.92;
security_guard_api_key security-secret;
privacy_guard_api_key privacy-secret;
usage_guard_api_key usage-secret;
audit_api_key audit-secret;
metrics_auth_token metrics-secret;
model_price model-a 0.10 0.30;
model_price model-b 0.20 0.40;
"#;
        fs::write(&path, text).expect("test config should be writable");
        let second = Config::from_file(&path).expect("test config should parse");
        fs::remove_file(path).ok();

        let first_hash = AssessmentContextV1::from_config(&first)
            .expect("context should serialize")
            .configuration_hash;
        let second_hash = AssessmentContextV1::from_config(&second)
            .expect("context should serialize")
            .configuration_hash;

        assert_eq!(first_hash, second_hash);
    }

    #[test]
    fn secret_changes_do_not_change_hash() {
        let first = load_config("");
        let mut second = first.clone();
        second.redis_url = "redis://user:changed-redis-secret@127.0.0.1:6379".to_string();
        second.upstream_api_key = "changed-upstream-secret".to_string();
        second.embedding_api_key = "changed-embedding-secret".to_string();
        second.qdrant_api_key = Some("changed-qdrant-secret".to_string());
        second.security_guard_api_key = Some("changed-security-secret".to_string());
        second.privacy_guard_api_key = Some("changed-privacy-secret".to_string());
        second.usage_guard_api_key = Some("changed-usage-secret".to_string());
        second.audit_api_key = Some("changed-audit-secret".to_string());
        second.metrics_auth_token = Some("changed-metrics-secret".to_string());

        let first_hash = AssessmentContextV1::from_config(&first)
            .expect("context should serialize")
            .configuration_hash;
        let second_hash = AssessmentContextV1::from_config(&second)
            .expect("context should serialize")
            .configuration_hash;

        assert_eq!(first_hash, second_hash);
    }

    #[test]
    fn routing_change_updates_hash_without_exposing_endpoint() {
        let first = load_config("");
        let mut second = first.clone();
        second.upstream_base_url = "http://other-chat.example:8000".to_string();

        let first_context =
            AssessmentContextV1::from_config(&first).expect("context should serialize");
        let second_context =
            AssessmentContextV1::from_config(&second).expect("context should serialize");

        assert_ne!(
            first_context.configuration_hash,
            second_context.configuration_hash
        );
        let json = serde_json::to_string(&second_context).expect("context JSON should serialize");
        assert!(!json.contains("other-chat.example"));
    }

    #[test]
    fn assessment_relevant_change_updates_hash() {
        let first = load_config("");
        let mut second = first.clone();
        second.semantic_similarity_threshold = 0.95;

        let first_hash = AssessmentContextV1::from_config(&first)
            .expect("context should serialize")
            .configuration_hash;
        let second_hash = AssessmentContextV1::from_config(&second)
            .expect("context should serialize")
            .configuration_hash;

        assert_ne!(first_hash, second_hash);
    }
}
