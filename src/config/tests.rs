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
        upstream_base_url: "https://api.openai.com".to_string(),
        upstream_api_key: "test-upstream-key".to_string(),
        embedding_base_url: "https://api.openai.com".to_string(),
        embedding_api_key: "test-embedding-key".to_string(),
        embedding_model: "text-embedding-3-small".to_string(),
        embedding_price: None,
        qdrant_url: "http://127.0.0.1:6334".to_string(),
        qdrant_api_key: None,
        qdrant_collection: "aif_semantic_cache".to_string(),
        qdrant_vector_size: 1536,
        cache_ttl_seconds: 86400,
        request_timeout_seconds: 120,
        graceful_shutdown_timeout_seconds: 10,
        max_request_body_bytes: 1_048_576,
        semantic_cache_enabled: false,
        semantic_similarity_threshold: 0.92,
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
}

#[test]
fn negative_embedding_price_fails_validation() {
    let cfg = Config {
        listen_addr: "127.0.0.1:8080".to_string(),
        redis_url: "redis://127.0.0.1:6379".to_string(),
        upstream_base_url: "https://api.openai.com".to_string(),
        upstream_api_key: "test-upstream-key".to_string(),
        embedding_base_url: "https://api.openai.com".to_string(),
        embedding_api_key: "test-embedding-key".to_string(),
        embedding_model: "text-embedding-3-small".to_string(),
        qdrant_url: "http://127.0.0.1:6334".to_string(),
        qdrant_api_key: None,
        qdrant_collection: "aif_semantic_cache".to_string(),
        qdrant_vector_size: 1536,
        cache_ttl_seconds: 86400,
        request_timeout_seconds: 120,
        graceful_shutdown_timeout_seconds: 10,
        max_request_body_bytes: 1_048_576,
        semantic_cache_enabled: false,
        semantic_similarity_threshold: 0.92,
        model_prices: HashMap::new(),
        embedding_price: Some(EmbeddingPrice {
            usd_per_1m_tokens: -0.020,
        }),
        allow_unknown_models_pass_through: false,
    };

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("embedding_price must be >= 0"));
}

#[test]
fn strict_model_validation_requires_model_price_or_passthrough() {
    let cfg = Config {
        listen_addr: "127.0.0.1:8080".to_string(),
        redis_url: "redis://127.0.0.1:6379".to_string(),
        upstream_base_url: "https://api.openai.com".to_string(),
        upstream_api_key: "test-upstream-key".to_string(),
        embedding_base_url: "https://api.openai.com".to_string(),
        embedding_api_key: "test-embedding-key".to_string(),
        embedding_model: "text-embedding-3-small".to_string(),
        embedding_price: None,
        qdrant_url: "http://127.0.0.1:6334".to_string(),
        qdrant_api_key: None,
        qdrant_collection: "aif_semantic_cache".to_string(),
        qdrant_vector_size: 1536,
        cache_ttl_seconds: 86400,
        request_timeout_seconds: 120,
        graceful_shutdown_timeout_seconds: 10,
        max_request_body_bytes: 1_048_576,
        semantic_cache_enabled: false,
        semantic_similarity_threshold: 0.92,
        model_prices: HashMap::new(),
        allow_unknown_models_pass_through: false,
    };

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("no models configured: either define at least one model_price or set allow_unknown_models_pass_through"));
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
    assert!(err.contains("embedding_api_key required"));
}

#[test]
fn semantic_cache_requires_qdrant_url() {
    let mut cfg = minimal_valid_config();
    cfg.semantic_cache_enabled = true;
    cfg.qdrant_url = "".to_string();

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("qdrant_url required"));
}

#[test]
fn invalid_listen_addr_fails_validation() {
    let mut cfg = minimal_valid_config();
    cfg.listen_addr = "not-an-addr".to_string();

    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("invalid listen_addr"));
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
