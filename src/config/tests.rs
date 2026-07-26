use super::*;
use serial_test::serial;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_config_path(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    std::env::temp_dir().join(format!("{}_{}.conf", name, nanos))
}

fn minimal_valid_config() -> Config {
    let mut prices = HashMap::new();
    prices.insert(
        "gpt-4o-mini-2024-07-18".to_string(),
        ModelPrice {
            input_usd_per_1m_tokens: 0.15,
            output_usd_per_1m_tokens: 0.60,
        },
    );

    Config {
        listen_addr: "127.0.0.1:8080".to_string(),
        redis_url: "redis://127.0.0.1:6379".to_string(),

        upstream_provider: ProviderKind::OpenAiCompatible,
        upstream_base_url: "https://api.openai.com".to_string(),
        upstream_api_key: "test-upstream-key".to_string(),

        embedding_provider: ProviderKind::OpenAiCompatible,
        embedding_base_url: "https://api.openai.com".to_string(),
        embedding_api_key: "test-embedding-key".to_string(),
        embedding_model: "text-embedding-3-small".to_string(),
        embedding_price: None,

        qdrant_url: "http://127.0.0.1:6334".to_string(),
        qdrant_api_key: None,
        qdrant_collection: "aif_semantic_cache".to_string(),
        qdrant_vector_size: 1536,

        cache_ttl_seconds: 86400,
        exact_cache_ttl_seconds: 86400,
        semantic_cache_retention_seconds: 86400,

        request_timeout_seconds: 120,
        upstream_timeout_seconds: 120,
        embedding_timeout_seconds: 30,
        graceful_shutdown_timeout_seconds: 10,
        max_request_body_bytes: 1_048_576,
        max_prompt_chars: 200_000,

        exact_cache_enabled: true,
        exact_cache_fail_open: true,
        exact_cache_store_enabled: true,

        semantic_cache_enabled: false,
        semantic_similarity_threshold: 0.92,
        semantic_cache_fail_open: true,
        semantic_cache_store_enabled: true,

        privacy_guard_enabled: false,
        privacy_guard_url: "http://127.0.0.1:8090".to_string(),
        privacy_guard_api_key: None,
        privacy_guard_mode: PrivacyGuardMode::DetectOnly,
        privacy_guard_restore_enabled: true,
        privacy_guard_tenant_id: None,
        privacy_guard_policy_id: None,
        privacy_guard_timeout_seconds: 10,
        guard_fail_open: true,

        audit_enabled: false,
        audit_url: "http://127.0.0.1:8092".to_string(),
        audit_api_key: None,
        audit_producer_instance_id: "test-instance".to_string(),
        audit_queue_capacity: 100,
        audit_batch_size: 10,
        audit_flush_interval_ms: 1_000,
        audit_timeout_seconds: 5,
        audit_retry_max_attempts: 3,
        audit_retry_initial_backoff_ms: 100,

        security_guard_enabled: false,
        security_guard_url: "http://vcal-security-guard:8091".to_string(),
        security_guard_api_key: None,
        security_guard_timeout_seconds: 3,

        cache_bypass_header: "X-AIF-Cache-Bypass".to_string(),
        metrics_auth_required: false,
        metrics_auth_token: None,

        readiness_requires_redis: true,
        readiness_requires_qdrant: false,
        readiness_requires_upstream: false,

        model_prices: prices,
        allow_unknown_models_pass_through: false,
    }
}

#[test]
fn parses_embedding_price_from_file() {
    let path = temp_config_path("aif_config_embedding_price");

    let text = r#"
listen_addr 127.0.0.1:8080;
redis_url redis://127.0.0.1:6379;
upstream_api_key test-upstream-key;
embedding_api_key test-embedding-key;
embedding_price 0.020;
qdrant_vector_size 1536;
cache_ttl_seconds 86400;
request_timeout_seconds 120;
semantic_cache_enabled false;
semantic_similarity_threshold 0.92;

model_price gpt-4o-mini-2024-07-18 0.15 0.60;
"#;

    fs::write(&path, text).unwrap();

    let cfg = Config::from_file(&path).unwrap();
    fs::remove_file(&path).ok();

    let embedding_price = cfg
        .embedding_price
        .expect("embedding_price should be parsed");
    assert!((embedding_price.usd_per_1m_tokens - 0.020).abs() < f64::EPSILON);
}

#[test]
#[serial]
fn parses_embedding_price_from_env() {
    unsafe {
        std::env::set_var("AIF_REDIS_URL", "redis://127.0.0.1:6379");
        std::env::set_var("AIF_UPSTREAM_API_KEY", "test-upstream-key");
        std::env::set_var("AIF_EMBEDDING_PRICE_USD_PER_1M_TOKENS", "0.020");
    }

    let cfg = Config::from_env().unwrap();

    let embedding_price = cfg
        .embedding_price
        .expect("embedding_price should be parsed");
    assert!((embedding_price.usd_per_1m_tokens - 0.020).abs() < f64::EPSILON);

    unsafe {
        std::env::remove_var("AIF_REDIS_URL");
        std::env::remove_var("AIF_UPSTREAM_API_KEY");
        std::env::remove_var("AIF_EMBEDDING_PRICE_USD_PER_1M_TOKENS");
    }
}

#[test]
#[serial]
fn invalid_embedding_price_in_env_is_rejected() {
    unsafe {
        std::env::set_var("AIF_REDIS_URL", "redis://127.0.0.1:6379");
        std::env::set_var("AIF_UPSTREAM_API_KEY", "test-upstream-key");
        std::env::set_var("AIF_EMBEDDING_PRICE_USD_PER_1M_TOKENS", "not-a-number");
    }

    let result = Config::from_env();

    unsafe {
        std::env::remove_var("AIF_REDIS_URL");
        std::env::remove_var("AIF_UPSTREAM_API_KEY");
        std::env::remove_var("AIF_EMBEDDING_PRICE_USD_PER_1M_TOKENS");
    }

    let err = result.unwrap_err().to_string();
    assert!(err.contains("invalid AIF_EMBEDDING_PRICE_USD_PER_1M_TOKENS"));
    assert!(err.contains("not-a-number"));
}

#[test]
fn negative_embedding_price_fails_validation() {
    let mut cfg = minimal_valid_config();
    cfg.embedding_price = Some(EmbeddingPrice {
        usd_per_1m_tokens: -0.020,
    });

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("embedding_price must be >= 0"));
}

#[test]
fn strict_model_validation_requires_model_price_or_passthrough() {
    let mut cfg = minimal_valid_config();
    cfg.model_prices = HashMap::new();
    cfg.allow_unknown_models_pass_through = false;

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("no allowed models configured"));
    assert!(err.contains("model_price"));
    assert!(err.contains("allow_unknown_models_pass_through=true"));
}

#[test]
fn invalid_max_request_body_bytes_in_file_is_rejected() {
    let path = temp_config_path("invalid_max_request_body_bytes");

    let text = r#"
listen_addr 127.0.0.1:8080;
redis_url redis://127.0.0.1:6379;
upstream_api_key test-upstream-key;
max_request_body_bytes abc;
cache_ttl_seconds 86400;
request_timeout_seconds 120;
semantic_cache_enabled false;
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
"#;

    fs::write(&path, text).unwrap();

    let result = Config::from_file(&path);
    fs::remove_file(&path).ok();

    let err = result.unwrap_err().to_string();
    assert!(err.contains("invalid byte size"));
}

#[test]
#[serial]
fn invalid_max_request_body_bytes_in_env_is_rejected() {
    unsafe {
        std::env::set_var("AIF_REDIS_URL", "redis://127.0.0.1:6379");
        std::env::set_var("AIF_UPSTREAM_API_KEY", "test-upstream-key");
        std::env::set_var("AIF_MAX_REQUEST_BODY_BYTES", "not-a-size");
    }

    let result = Config::from_env();

    unsafe {
        std::env::remove_var("AIF_REDIS_URL");
        std::env::remove_var("AIF_UPSTREAM_API_KEY");
        std::env::remove_var("AIF_MAX_REQUEST_BODY_BYTES");
    }

    let err = result.unwrap_err().to_string();
    assert!(err.contains("invalid AIF_MAX_REQUEST_BODY_BYTES"));
    assert!(err.contains("not-a-size"));
}

#[test]
#[serial]
fn missing_required_env_vars_are_reported_clearly() {
    unsafe {
        std::env::remove_var("AIF_REDIS_URL");
        std::env::remove_var("AIF_UPSTREAM_API_KEY");
    }

    let result = Config::from_env();
    let err = result.unwrap_err().to_string();

    assert!(err.contains("AIF_REDIS_URL is required when no config file is used"));
}

#[test]
fn zero_max_request_body_bytes_fails_validation() {
    let mut cfg = minimal_valid_config();
    cfg.max_request_body_bytes = 0;

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("max_request_body_bytes must be > 0"));
}

#[test]
fn zero_cache_ttl_seconds_fails_validation() {
    let mut cfg = minimal_valid_config();
    cfg.cache_ttl_seconds = 0;

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("cache_ttl_seconds must be > 0"));
}

#[test]
fn zero_request_timeout_seconds_fails_validation() {
    let mut cfg = minimal_valid_config();
    cfg.request_timeout_seconds = 0;

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("request_timeout_seconds must be > 0"));
}

#[test]
fn zero_graceful_shutdown_timeout_seconds_fails_validation() {
    let mut cfg = minimal_valid_config();
    cfg.graceful_shutdown_timeout_seconds = 0;

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("graceful_shutdown_timeout_seconds must be > 0"));
}

#[test]
fn similarity_threshold_above_one_fails_validation() {
    let mut cfg = minimal_valid_config();
    cfg.semantic_similarity_threshold = 1.1;

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("semantic_similarity_threshold must be between 0.0 and 1.0"));
}

#[test]
fn similarity_threshold_below_zero_fails_validation() {
    let mut cfg = minimal_valid_config();
    cfg.semantic_similarity_threshold = -0.1;

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("semantic_similarity_threshold must be between 0.0 and 1.0"));
}

#[test]
fn semantic_cache_requires_embedding_api_key() {
    let mut cfg = minimal_valid_config();
    cfg.semantic_cache_enabled = true;
    cfg.embedding_api_key = "".to_string();

    let err = cfg.validate().unwrap_err().to_string();

    assert!(err.contains("embedding_api_key must not be empty when semantic_cache_enabled=true"));
    assert!(err.contains("dummy"));
    assert!(err.contains("none"));
    assert!(err.contains("null"));
    assert!(err.contains("-"));
    assert!(!err.contains("semantic_cache_enabled=true requires: embedding_api_key"));
}

#[test]
fn semantic_cache_requires_qdrant_url() {
    let mut cfg = minimal_valid_config();
    cfg.semantic_cache_enabled = true;
    cfg.qdrant_url = "".to_string();

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("semantic_cache_enabled=true requires:"));
    assert!(err.contains("qdrant_url"));
}

#[test]
fn semantic_cache_reports_multiple_missing_fields_together() {
    let mut cfg = minimal_valid_config();
    cfg.semantic_cache_enabled = true;
    cfg.embedding_api_key = "".to_string();
    cfg.embedding_model = "".to_string();
    cfg.qdrant_url = "".to_string();
    cfg.qdrant_collection = "".to_string();

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("semantic_cache_enabled=true requires:"));
    assert!(err.contains("embedding_api_key"));
    assert!(err.contains("embedding_model"));
    assert!(err.contains("qdrant_url"));
    assert!(err.contains("qdrant_collection"));
}

#[test]
fn provider_fields_default_to_openai_compatible() {
    let path = temp_config_path("provider_defaults");

    let text = r#"
listen_addr 127.0.0.1:8080;
redis_url redis://127.0.0.1:6379;
upstream_api_key test-upstream-key;
embedding_api_key test-embedding-key;
cache_ttl_seconds 86400;
request_timeout_seconds 120;
semantic_cache_enabled false;
semantic_similarity_threshold 0.92;
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
"#;

    fs::write(&path, text).unwrap();

    let cfg = Config::from_file(&path).unwrap();
    fs::remove_file(&path).ok();

    assert_eq!(cfg.upstream_provider, ProviderKind::OpenAiCompatible);
    assert_eq!(cfg.embedding_provider, ProviderKind::OpenAiCompatible);
    assert!(cfg.semantic_cache_fail_open);
}

#[test]
fn parses_provider_fields_and_semantic_fail_open_from_file() {
    let path = temp_config_path("provider_fields");

    let text = r#"
listen_addr 127.0.0.1:8080;
redis_url redis://127.0.0.1:6379;

upstream_provider openai_compatible;
upstream_api_key test-upstream-key;

embedding_provider openai_compatible;
embedding_api_key test-embedding-key;

semantic_cache_fail_open false;

cache_ttl_seconds 86400;
request_timeout_seconds 120;
semantic_cache_enabled false;
semantic_similarity_threshold 0.92;

model_price gpt-4o-mini-2024-07-18 0.15 0.60;
"#;

    fs::write(&path, text).unwrap();

    let cfg = Config::from_file(&path).unwrap();
    fs::remove_file(&path).ok();

    assert_eq!(cfg.upstream_provider, ProviderKind::OpenAiCompatible);
    assert_eq!(cfg.embedding_provider, ProviderKind::OpenAiCompatible);
    assert!(!cfg.semantic_cache_fail_open);
}

#[test]
fn invalid_upstream_provider_is_rejected() {
    let path = temp_config_path("invalid_upstream_provider");

    let text = r#"
listen_addr 127.0.0.1:8080;
redis_url redis://127.0.0.1:6379;
upstream_provider invalid_provider;
upstream_api_key test-upstream-key;
cache_ttl_seconds 86400;
request_timeout_seconds 120;
semantic_cache_enabled false;
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
"#;

    fs::write(&path, text).unwrap();

    let result = Config::from_file(&path);
    fs::remove_file(&path).ok();

    let err = result.unwrap_err().to_string();
    assert!(err.contains("invalid value for upstream_provider"));
    assert!(err.contains("unsupported provider"));
}

#[test]
fn invalid_embedding_provider_is_rejected() {
    let path = temp_config_path("invalid_embedding_provider");

    let text = r#"
listen_addr 127.0.0.1:8080;
redis_url redis://127.0.0.1:6379;
upstream_api_key test-upstream-key;
embedding_provider invalid_provider;
cache_ttl_seconds 86400;
request_timeout_seconds 120;
semantic_cache_enabled false;
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
"#;

    fs::write(&path, text).unwrap();

    let result = Config::from_file(&path);
    fs::remove_file(&path).ok();

    let err = result.unwrap_err().to_string();
    assert!(err.contains("invalid value for embedding_provider"));
    assert!(err.contains("unsupported provider"));
}

#[test]
#[serial]
fn parses_provider_fields_and_semantic_fail_open_from_env() {
    unsafe {
        std::env::set_var("AIF_REDIS_URL", "redis://127.0.0.1:6379");
        std::env::set_var("AIF_UPSTREAM_API_KEY", "test-upstream-key");
        std::env::set_var("AIF_UPSTREAM_PROVIDER", "openai_compatible");
        std::env::set_var("AIF_EMBEDDING_PROVIDER", "openai_compatible");
        std::env::set_var("AIF_SEMANTIC_CACHE_FAIL_OPEN", "false");
    }

    let cfg = Config::from_env().unwrap();

    unsafe {
        std::env::remove_var("AIF_REDIS_URL");
        std::env::remove_var("AIF_UPSTREAM_API_KEY");
        std::env::remove_var("AIF_UPSTREAM_PROVIDER");
        std::env::remove_var("AIF_EMBEDDING_PROVIDER");
        std::env::remove_var("AIF_SEMANTIC_CACHE_FAIL_OPEN");
    }

    assert_eq!(cfg.upstream_provider, ProviderKind::OpenAiCompatible);
    assert_eq!(cfg.embedding_provider, ProviderKind::OpenAiCompatible);
    assert!(!cfg.semantic_cache_fail_open);
}

#[test]
fn invalid_listen_addr_fails_validation() {
    let mut cfg = minimal_valid_config();
    cfg.listen_addr = "not-an-addr".to_string();

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("invalid listen_addr"));
}

#[test]
fn invalid_redis_url_prefix_fails_validation() {
    let mut cfg = minimal_valid_config();
    cfg.redis_url = "http://127.0.0.1:6379".to_string();

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("invalid redis_url"));
    assert!(err.contains("must start with redis://"));
}

#[test]
fn empty_upstream_api_key_fails_validation() {
    let mut cfg = minimal_valid_config();
    cfg.upstream_api_key = "".to_string();

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("upstream_api_key must not be empty"));
}

#[test]
fn zero_qdrant_vector_size_fails_validation() {
    let mut cfg = minimal_valid_config();
    cfg.qdrant_vector_size = 0;

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("qdrant_vector_size must be > 0"));
}

#[test]
fn missing_semicolon_in_file_is_rejected() {
    let path = temp_config_path("missing_semicolon");

    let text = r#"
listen_addr 127.0.0.1:8080
redis_url redis://127.0.0.1:6379;
upstream_api_key test-upstream-key;
"#;

    fs::write(&path, text).unwrap();

    let result = Config::from_file(&path);
    fs::remove_file(&path).ok();

    let err = result.unwrap_err().to_string();
    assert!(err.contains("missing ';'"));
}

#[test]
fn unknown_directive_is_rejected() {
    let path = temp_config_path("unknown_directive");

    let text = r#"
listen_addr 127.0.0.1:8080;
redis_url redis://127.0.0.1:6379;
upstream_api_key test-upstream-key;
unknown_key value;
"#;

    fs::write(&path, text).unwrap();

    let result = Config::from_file(&path);
    fs::remove_file(&path).ok();

    let err = result.unwrap_err().to_string();
    assert!(err.contains("unknown directive"));
}

#[test]
fn duplicate_directive_is_rejected() {
    let path = temp_config_path("duplicate_directive");

    let text = r#"
listen_addr 127.0.0.1:8080;
listen_addr 127.0.0.1:8081;
redis_url redis://127.0.0.1:6379;
upstream_api_key test-upstream-key;
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
"#;

    fs::write(&path, text).unwrap();

    let result = Config::from_file(&path);
    fs::remove_file(&path).ok();

    let err = result.unwrap_err().to_string();
    assert!(err.contains("duplicate directive"));
}

#[test]
fn duplicate_model_price_is_rejected() {
    let path = temp_config_path("duplicate_model_price");

    let text = r#"
listen_addr 127.0.0.1:8080;
redis_url redis://127.0.0.1:6379;
upstream_api_key test-upstream-key;
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
model_price gpt-4o-mini-2024-07-18 0.20 0.70;
"#;

    fs::write(&path, text).unwrap();

    let result = Config::from_file(&path);
    fs::remove_file(&path).ok();

    let err = result.unwrap_err().to_string();
    assert!(err.contains("duplicate model_price"));
}

#[test]
fn invalid_model_price_input_is_rejected() {
    let path = temp_config_path("invalid_model_price_input");

    let text = r#"
listen_addr 127.0.0.1:8080;
redis_url redis://127.0.0.1:6379;
upstream_api_key test-upstream-key;
model_price gpt-4o-mini-2024-07-18 nope 0.60;
"#;

    fs::write(&path, text).unwrap();

    let result = Config::from_file(&path);
    fs::remove_file(&path).ok();

    let err = result.unwrap_err().to_string();
    assert!(err.contains("invalid model_price input price"));
}

#[test]
fn exact_cache_ttl_defaults_to_legacy_cache_ttl() {
    let path = temp_config_path("exact_ttl_default");

    let text = r#"
listen_addr 127.0.0.1:8080;
redis_url redis://127.0.0.1:6379;
upstream_api_key test-upstream-key;
cache_ttl_seconds 86400;
request_timeout_seconds 120;
semantic_cache_enabled false;
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
"#;

    fs::write(&path, text).unwrap();

    let cfg = Config::from_file(&path).unwrap();
    fs::remove_file(&path).ok();

    assert_eq!(cfg.exact_cache_ttl_seconds, 86400);
}

#[test]
fn openai_compatible_base_urls_accept_root_and_v1() {
    for url in [
        "https://api.openai.com",
        "https://api.openai.com/v1",
        "http://ollama:11434",
        "http://ollama:11434/v1",
        "http://lmstudio:1234/v1",
        "http://vllm:8000/v1",
        "http://litellm:4000/v1",
    ] {
        let mut cfg = minimal_valid_config();
        cfg.upstream_base_url = url.to_string();
        cfg.embedding_base_url = url.to_string();
        cfg.semantic_cache_enabled = true;

        assert!(
            cfg.validate().is_ok(),
            "expected valid OpenAI-compatible base URL: {url}"
        );
    }
}

#[test]
fn openai_compatible_base_url_rejects_missing_scheme() {
    let mut cfg = minimal_valid_config();
    cfg.upstream_base_url = "localhost:11434/v1".to_string();

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("upstream_base_url"));
    assert!(err.contains("http:// or https://"));
}

#[test]
fn openai_compatible_base_url_rejects_full_chat_endpoint() {
    let mut cfg = minimal_valid_config();
    cfg.upstream_base_url = "http://localhost:11434/v1/chat/completions".to_string();

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("upstream_base_url"));
    assert!(err.contains("not a full endpoint path"));
}

#[test]
fn openai_compatible_base_url_rejects_full_embeddings_endpoint() {
    let mut cfg = minimal_valid_config();
    cfg.semantic_cache_enabled = true;
    cfg.embedding_base_url = "http://localhost:11434/v1/embeddings".to_string();

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("embedding_base_url"));
    assert!(err.contains("not a full endpoint path"));
}

#[test]
fn empty_upstream_api_key_suggests_placeholder_values() {
    let mut cfg = minimal_valid_config();
    cfg.upstream_api_key = "".to_string();

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("upstream_api_key must not be empty"));
    assert!(err.contains("dummy"));
}

#[test]
fn parses_local_openai_compatible_provider_with_dummy_keys() {
    let path = temp_config_path("local_openai_compatible");

    let text = r#"
listen_addr 127.0.0.1:8080;
redis_url redis://127.0.0.1:6379;

upstream_provider openai_compatible;
upstream_base_url http://ollama:11434/v1;
upstream_api_key dummy;

embedding_provider openai_compatible;
embedding_base_url http://ollama:11434/v1;
embedding_api_key dummy;
embedding_model nomic-embed-text;

qdrant_url http://127.0.0.1:6334;
qdrant_collection aif_semantic_cache;
qdrant_vector_size 768;

cache_ttl_seconds 86400;
request_timeout_seconds 120;
semantic_cache_enabled true;
semantic_similarity_threshold 0.92;

allow_unknown_models_pass_through true;
"#;

    fs::write(&path, text).unwrap();

    let cfg = Config::from_file(&path).unwrap();
    fs::remove_file(&path).ok();

    assert_eq!(cfg.upstream_provider, ProviderKind::OpenAiCompatible);
    assert_eq!(cfg.upstream_base_url, "http://ollama:11434/v1");
    assert_eq!(cfg.upstream_api_key, "dummy");
    assert_eq!(cfg.embedding_base_url, "http://ollama:11434/v1");
    assert_eq!(cfg.embedding_api_key, "dummy");
    assert_eq!(cfg.qdrant_vector_size, 768);
    assert!(cfg.semantic_cache_enabled);
}

#[test]
fn parses_audit_settings_from_file() {
    let path = temp_config_path("aif_config_audit");

    let text = r#"
listen_addr 127.0.0.1:8080;
redis_url redis://127.0.0.1:6379;
upstream_api_key test-upstream-key;
semantic_cache_enabled false;
audit_enabled true;
audit_url http://127.0.0.1:8092;
audit_api_key test-audit-token;
audit_producer_instance_id test-firewall-01;
audit_queue_capacity 500;
audit_batch_size 50;
audit_flush_interval_ms 750;
audit_timeout_seconds 4;
audit_retry_max_attempts 6;
audit_retry_initial_backoff_ms 125;
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
"#;

    fs::write(&path, text).unwrap();
    let cfg = Config::from_file(&path).unwrap();
    fs::remove_file(&path).ok();

    assert!(cfg.audit_enabled);
    assert_eq!(cfg.audit_url, "http://127.0.0.1:8092");
    assert_eq!(cfg.audit_api_key.as_deref(), Some("test-audit-token"));
    assert_eq!(cfg.audit_producer_instance_id, "test-firewall-01");
    assert_eq!(cfg.audit_queue_capacity, 500);
    assert_eq!(cfg.audit_batch_size, 50);
    assert_eq!(cfg.audit_flush_interval_ms, 750);
    assert_eq!(cfg.audit_timeout_seconds, 4);
    assert_eq!(cfg.audit_retry_max_attempts, 6);
    assert_eq!(cfg.audit_retry_initial_backoff_ms, 125);
}

#[test]
fn audit_batch_size_cannot_exceed_queue_capacity() {
    let mut cfg = minimal_valid_config();
    cfg.audit_enabled = true;
    cfg.audit_queue_capacity = 10;
    cfg.audit_batch_size = 11;

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("audit_batch_size must not exceed audit_queue_capacity"));
}
