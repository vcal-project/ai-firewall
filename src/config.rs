use anyhow::{anyhow, Context, Result};
use std::{
    collections::{HashMap, HashSet},
    env, fmt, fs,
    path::Path,
};

fn cfg_err(msg: impl Into<String>) -> anyhow::Error {
    anyhow!("configuration error: {}", msg.into())
}

fn warn_if_suspicious(cfg: &Config) {
    if cfg.max_request_body_bytes < 1024 {
        tracing::warn!(
            "max_request_body_bytes={} is very small; requests larger than this will be rejected. Consider using at least 1K, for example 512K or 1M",
            cfg.max_request_body_bytes
        );
    }
}

#[derive(Clone, Debug)]
pub struct ModelPrice {
    pub input_usd_per_1m_tokens: f64,
    pub output_usd_per_1m_tokens: f64,
}

#[derive(Clone, Debug)]
pub struct EmbeddingPrice {
    pub usd_per_1m_tokens: f64,
}

#[derive(Clone)]
pub struct Config {
    pub listen_addr: String,
    pub redis_url: String,

    pub upstream_base_url: String,
    pub upstream_api_key: String,

    pub embedding_base_url: String,
    pub embedding_api_key: String,
    pub embedding_model: String,
    pub embedding_price: Option<EmbeddingPrice>,

    pub qdrant_url: String,
    pub qdrant_api_key: Option<String>,
    pub qdrant_collection: String,
    pub qdrant_vector_size: u64,

    pub cache_ttl_seconds: usize,
    pub request_timeout_seconds: u64,
    pub graceful_shutdown_timeout_seconds: u64,
    pub max_request_body_bytes: usize,

    pub semantic_cache_enabled: bool,
    pub semantic_similarity_threshold: f32,

    pub model_prices: HashMap<String, ModelPrice>,

    pub allow_unknown_models_pass_through: bool,
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        let mut errors: Vec<String> = Vec::new();

        // ---- listen_addr
        if let Err(e) = self.listen_addr.parse::<std::net::SocketAddr>() {
            errors.push(format!("invalid listen_addr '{}': {}", self.listen_addr, e));
        }

        // ---- redis
        if self.redis_url.trim().is_empty() {
            errors.push("redis_url must not be empty".into());
        } else if !self.redis_url.starts_with("redis://") {
            errors.push(format!(
                "invalid redis_url '{}': must start with redis://",
                self.redis_url
            ));
        }

        // ---- upstream
        if self.upstream_base_url.trim().is_empty() {
            errors.push("upstream_base_url must not be empty".into());
        }

        if self.upstream_api_key.trim().is_empty() {
            errors.push("upstream_api_key must not be empty".into());
        }

        // ---- timeouts
        if self.request_timeout_seconds == 0 {
            errors.push("request_timeout_seconds must be > 0".into());
        }

        if self.cache_ttl_seconds == 0 {
            errors.push("cache_ttl_seconds must be > 0".into());
        }

        if self.graceful_shutdown_timeout_seconds == 0 {
            errors.push("graceful_shutdown_timeout_seconds must be > 0".into());
        }

        // ---- request size
        if self.max_request_body_bytes == 0 {
            errors.push("max_request_body_bytes must be > 0 (example: 1M, 512K, 1048576)".into());
        }

        // ---- semantic threshold
        if !(0.0..=1.0).contains(&self.semantic_similarity_threshold) {
            errors.push(format!(
                "semantic_similarity_threshold must be between 0.0 and 1.0, got {}",
                self.semantic_similarity_threshold
            ));
        }

        // ---- qdrant
        if self.qdrant_vector_size == 0 {
            errors.push("qdrant_vector_size must be > 0".into());
        }

        // ---- semantic cache block (aggregated)
        if self.semantic_cache_enabled {
            let mut missing = Vec::new();

            if self.embedding_base_url.trim().is_empty() {
                missing.push("embedding_base_url");
            }
            if self.embedding_api_key.trim().is_empty() {
                missing.push("embedding_api_key");
            }
            if self.embedding_model.trim().is_empty() {
                missing.push("embedding_model");
            }
            if self.qdrant_url.trim().is_empty() {
                missing.push("qdrant_url");
            }
            if self.qdrant_collection.trim().is_empty() {
                missing.push("qdrant_collection");
            }

            if !missing.is_empty() {
                errors.push(format!(
                    "semantic_cache_enabled=true requires: {}",
                    missing.join(", ")
                ));
            }
        }

        // ---- model pricing
        for (model, price) in &self.model_prices {
            if model.trim().is_empty() {
                errors.push("model_prices contains an empty model name".into());
                continue;
            }

            if !price.input_usd_per_1m_tokens.is_finite()
                || !price.output_usd_per_1m_tokens.is_finite()
            {
                errors.push(format!(
                    "model_price '{}' must have finite input/output values",
                    model
                ));
            }

            if price.input_usd_per_1m_tokens < 0.0 || price.output_usd_per_1m_tokens < 0.0 {
                errors.push(format!(
                    "model_price '{}' must be >= 0 for both input and output",
                    model
                ));
            }
        }

        // ---- embedding price
        if let Some(price) = &self.embedding_price {
            if !price.usd_per_1m_tokens.is_finite() {
                errors.push("embedding_price must be finite".into());
            }

            if price.usd_per_1m_tokens < 0.0 {
                errors.push("embedding_price must be >= 0".into());
            }
        }

        // ---- model allowlist logic (important improvement)
        if !self.allow_unknown_models_pass_through && self.model_prices.is_empty() {
            errors.push(
                "no allowed models configured: add at least one `model_price <model> <input> <output>` \
                 or set allow_unknown_models_pass_through=true"
                    .into(),
            );
        }

        // ---- FINAL
        if !errors.is_empty() {
            return Err(cfg_err(errors.join("; ")));
        }

        Ok(())
    }

    fn parse_bytes(input: &str) -> Result<usize> {
        let s = input.trim();
        if s.is_empty() {
            return Err(cfg_err(
                "byte size must not be empty; use formats like 1024, 512K, 1M, 2M",
            ));
        }

        let upper = s.to_ascii_uppercase();

        let (number_part, multiplier) = if let Some(num) = upper.strip_suffix("KB") {
            (num, 1024usize)
        } else if let Some(num) = upper.strip_suffix('K') {
            (num, 1024usize)
        } else if let Some(num) = upper.strip_suffix("MB") {
            (num, 1024usize * 1024)
        } else if let Some(num) = upper.strip_suffix('M') {
            (num, 1024usize * 1024)
        } else if let Some(num) = upper.strip_suffix("GB") {
            (num, 1024usize * 1024 * 1024)
        } else if let Some(num) = upper.strip_suffix('G') {
            (num, 1024usize * 1024 * 1024)
        } else {
            (upper.as_str(), 1usize)
        };

        let base: usize = number_part.trim().parse().map_err(|_| {
            cfg_err(format!(
                "invalid byte size '{}'. Use formats like 1024, 512K, 1M, 2M",
                input
            ))
        })?;

        base.checked_mul(multiplier)
            .ok_or_else(|| cfg_err(format!("byte size is too large: '{}'", input)))
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file {}", path.as_ref().display()))?;

        let map = parse_nginx_style_config(&text)?;
        let model_prices = parse_model_prices(&text)?;

        let cfg = Self {
            listen_addr: get_or_default(&map, "listen_addr", "0.0.0.0:8080"),
            redis_url: get_required(&map, "redis_url")?,

            upstream_base_url: get_or_default(&map, "upstream_base_url", "https://api.openai.com"),
            upstream_api_key: get_required(&map, "upstream_api_key")?,

            embedding_base_url: get_or_default(
                &map,
                "embedding_base_url",
                "https://api.openai.com",
            ),
            embedding_api_key: map
                .get("embedding_api_key")
                .cloned()
                .unwrap_or_else(|| map.get("upstream_api_key").cloned().unwrap_or_default()),
            embedding_model: get_or_default(&map, "embedding_model", "text-embedding-3-small"),

            embedding_price: map
                .get("embedding_price")
                .map(|v| {
                    v.parse::<f64>()
                        .map(|usd_per_1m_tokens| EmbeddingPrice { usd_per_1m_tokens })
                        .map_err(|e| {
                            cfg_err(format!("invalid embedding_price value '{}': {}", v, e))
                        })
                })
                .transpose()?,

            qdrant_url: get_or_default(&map, "qdrant_url", "http://127.0.0.1:6334"),
            qdrant_api_key: map.get("qdrant_api_key").cloned(),
            qdrant_collection: get_or_default(&map, "qdrant_collection", "aif_semantic_cache"),
            qdrant_vector_size: parse_or_default(&map, "qdrant_vector_size", 1536u64)?,

            cache_ttl_seconds: parse_or_default(&map, "cache_ttl_seconds", 86400usize)?,
            request_timeout_seconds: parse_or_default(&map, "request_timeout_seconds", 120u64)?,

            semantic_cache_enabled: parse_or_default(&map, "semantic_cache_enabled", false)?,
            semantic_similarity_threshold: parse_or_default(
                &map,
                "semantic_similarity_threshold",
                0.92f32,
            )?,

            model_prices,

            allow_unknown_models_pass_through: parse_or_default(
                &map,
                "allow_unknown_models_pass_through",
                false,
            )?,

            graceful_shutdown_timeout_seconds: parse_or_default(
                &map,
                "graceful_shutdown_timeout_seconds",
                10u64,
            )?,

            max_request_body_bytes: map
                .get("max_request_body_bytes")
                .map(|v| Self::parse_bytes(v))
                .transpose()?
                .unwrap_or(1_048_576usize),
        };

        warn_if_suspicious(&cfg);

        Ok(cfg)
    }

    pub fn from_env() -> Result<Self> {
        let cfg = Self {
            listen_addr: env::var("AIF_LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            redis_url: env::var("AIF_REDIS_URL")
                .map_err(|_| cfg_err("AIF_REDIS_URL is required when no config file is used"))?,

            upstream_base_url: env::var("AIF_UPSTREAM_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".into()),
            upstream_api_key: env::var("AIF_UPSTREAM_API_KEY").map_err(|_| {
                cfg_err("AIF_UPSTREAM_API_KEY is required when no config file is used")
            })?,

            embedding_base_url: env::var("AIF_EMBEDDING_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".into()),
            embedding_api_key: env::var("AIF_EMBEDDING_API_KEY")
                .unwrap_or_else(|_| env::var("AIF_UPSTREAM_API_KEY").unwrap_or_default()),
            embedding_model: env::var("AIF_EMBEDDING_MODEL")
                .unwrap_or_else(|_| "text-embedding-3-small".into()),

            embedding_price: env::var("AIF_EMBEDDING_PRICE_USD_PER_1M_TOKENS")
                .ok()
                .map(|v| {
                    v.parse::<f64>().map_err(|e| {
                        cfg_err(format!(
                            "invalid AIF_EMBEDDING_PRICE_USD_PER_1M_TOKENS value '{}': {}",
                            v, e
                        ))
                    })
                })
                .transpose()?
                .map(|usd_per_1m_tokens| EmbeddingPrice { usd_per_1m_tokens }),

            qdrant_url: env::var("AIF_QDRANT_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:6334".into()),
            qdrant_api_key: env::var("AIF_QDRANT_API_KEY").ok(),
            qdrant_collection: env::var("AIF_QDRANT_COLLECTION")
                .unwrap_or_else(|_| "aif_semantic_cache".into()),
            qdrant_vector_size: {
                let raw = env::var("AIF_QDRANT_VECTOR_SIZE").unwrap_or_else(|_| "1536".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_QDRANT_VECTOR_SIZE value '{}': {}",
                        raw, e
                    ))
                })?
            },

            cache_ttl_seconds: {
                let raw = env::var("AIF_CACHE_TTL_SECONDS").unwrap_or_else(|_| "86400".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_CACHE_TTL_SECONDS value '{}': {}",
                        raw, e
                    ))
                })?
            },

            request_timeout_seconds: {
                let raw = env::var("AIF_REQUEST_TIMEOUT_SECONDS").unwrap_or_else(|_| "120".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_REQUEST_TIMEOUT_SECONDS value '{}': {}",
                        raw, e
                    ))
                })?
            },

            semantic_cache_enabled: {
                let raw = env::var("AIF_SEMANTIC_CACHE_ENABLED").unwrap_or_else(|_| "false".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_SEMANTIC_CACHE_ENABLED value '{}': {}",
                        raw, e
                    ))
                })?
            },

            semantic_similarity_threshold: {
                let raw =
                    env::var("AIF_SEMANTIC_SIMILARITY_THRESHOLD").unwrap_or_else(|_| "0.92".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_SEMANTIC_SIMILARITY_THRESHOLD value '{}': {}",
                        raw, e
                    ))
                })?
            },

            model_prices: HashMap::new(),

            allow_unknown_models_pass_through: {
                let raw = env::var("AIF_ALLOW_UNKNOWN_MODELS_PASS_THROUGH")
                    .unwrap_or_else(|_| "false".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_ALLOW_UNKNOWN_MODELS_PASS_THROUGH value '{}': {}",
                        raw, e
                    ))
                })?
            },

            graceful_shutdown_timeout_seconds: {
                let raw = env::var("AIF_GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS")
                    .unwrap_or_else(|_| "10".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS value '{}': {}",
                        raw, e
                    ))
                })?
            },

            max_request_body_bytes: {
                let raw = env::var("AIF_MAX_REQUEST_BODY_BYTES").unwrap_or_else(|_| "1M".into());
                Self::parse_bytes(&raw).map_err(|_| {
                    cfg_err(format!(
                        "invalid AIF_MAX_REQUEST_BODY_BYTES value '{}'. Use formats like 1024, 512K, 1M, 2M",
                        raw
                    ))
                })?
            },
        };

        warn_if_suspicious(&cfg);

        Ok(cfg)
    }

    pub fn from_env_or_file(path: Option<&str>) -> Result<Self> {
        if let Some(p) = path {
            tracing::info!("loading config file {}", p);
            return Self::from_file(p);
        }

        let candidates = [
            "configs/ai-firewall.conf",
            "/etc/ai-firewall/ai-firewall.conf",
        ];

        for p in candidates {
            if std::path::Path::new(p).exists() {
                tracing::info!("loading config file {}", p);
                return Self::from_file(p);
            }
        }

        tracing::info!("no config file found, falling back to environment variables");
        Self::from_env()
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("listen_addr", &self.listen_addr)
            .field("redis_url", &self.redis_url)
            .field("upstream_base_url", &self.upstream_base_url)
            .field("upstream_api_key", &mask_secret(&self.upstream_api_key))
            .field("embedding_base_url", &self.embedding_base_url)
            .field("embedding_api_key", &mask_secret(&self.embedding_api_key))
            .field("embedding_model", &self.embedding_model)
            .field("embedding_price", &self.embedding_price)
            .field("qdrant_url", &self.qdrant_url)
            .field(
                "qdrant_api_key",
                &self.qdrant_api_key.as_ref().map(|k| mask_secret(k)),
            )
            .field("qdrant_collection", &self.qdrant_collection)
            .field("qdrant_vector_size", &self.qdrant_vector_size)
            .field("cache_ttl_seconds", &self.cache_ttl_seconds)
            .field("request_timeout_seconds", &self.request_timeout_seconds)
            .field("semantic_cache_enabled", &self.semantic_cache_enabled)
            .field(
                "semantic_similarity_threshold",
                &self.semantic_similarity_threshold,
            )
            .field("model_prices", &self.model_prices)
            .field(
                "allow_unknown_models_pass_through",
                &self.allow_unknown_models_pass_through,
            )
            .field(
                "graceful_shutdown_timeout_seconds",
                &self.graceful_shutdown_timeout_seconds,
            )
            .field("max_request_body_bytes", &self.max_request_body_bytes)
            .finish()
    }
}

fn mask_secret(s: &str) -> String {
    let s = s.trim();

    if s.is_empty() {
        return "<empty>".into();
    }

    if s.chars().count() <= 8 {
        return "****".into();
    }

    let prefix: String = s.chars().take(4).collect();
    format!("{prefix}****")
}

fn allowed_directives() -> HashSet<&'static str> {
    HashSet::from([
        "listen_addr",
        "redis_url",
        "upstream_base_url",
        "upstream_api_key",
        "embedding_base_url",
        "embedding_api_key",
        "embedding_model",
        "embedding_price",
        "qdrant_url",
        "qdrant_api_key",
        "qdrant_collection",
        "qdrant_vector_size",
        "cache_ttl_seconds",
        "request_timeout_seconds",
        "semantic_cache_enabled",
        "semantic_similarity_threshold",
        "model_price",
        "allow_unknown_models_pass_through",
        "graceful_shutdown_timeout_seconds",
        "max_request_body_bytes",
    ])
}

fn parse_model_prices(input: &str) -> Result<HashMap<String, ModelPrice>> {
    let mut prices = HashMap::new();

    for (idx, raw_line) in input.lines().enumerate() {
        let line_no = idx + 1;

        let line = raw_line.split('#').next().unwrap_or("").trim();

        if line.is_empty() {
            continue;
        }

        if !line.ends_with(';') {
            continue;
        }

        let line = line.trim_end_matches(';').trim();
        let parts: Vec<&str> = line.split_whitespace().collect();

        let [directive, model_raw, input_raw, output_raw] = parts.as_slice() else {
            if parts.first().copied() == Some("model_price") {
                return Err(anyhow!(
                    "config parse error on line {line_no}: model_price requires 3 values: <model> <input_usd_per_1m_tokens> <output_usd_per_1m_tokens>"
                ));
            }
            continue;
        };

        if *directive != "model_price" {
            continue;
        }

        let model = strip_quotes(model_raw.trim());
        if model.trim().is_empty() {
            return Err(anyhow!(
                "config parse error on line {line_no}: model_price model must not be empty"
            ));
        }

        let input_price = input_raw.parse::<f64>().map_err(|e| {
            anyhow!("config parse error on line {line_no}: invalid model_price input price: {e}")
        })?;

        let output_price = output_raw.parse::<f64>().map_err(|e| {
            anyhow!("config parse error on line {line_no}: invalid model_price output price: {e}")
        })?;

        if !input_price.is_finite() || !output_price.is_finite() {
            return Err(anyhow!(
                "config parse error on line {line_no}: model_price values must be finite"
            ));
        }

        if input_price < 0.0 || output_price < 0.0 {
            return Err(anyhow!(
                "config parse error on line {line_no}: model_price values must be >= 0"
            ));
        }

        if prices.contains_key(&model) {
            return Err(anyhow!(
                "config parse error on line {line_no}: duplicate model_price for model '{}'",
                model
            ));
        }

        prices.insert(
            model,
            ModelPrice {
                input_usd_per_1m_tokens: input_price,
                output_usd_per_1m_tokens: output_price,
            },
        );
    }

    Ok(prices)
}

fn parse_nginx_style_config(input: &str) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    let allowed = allowed_directives();

    for (idx, raw_line) in input.lines().enumerate() {
        let line_no = idx + 1;

        let line = raw_line.split('#').next().unwrap_or("").trim();

        if line.is_empty() {
            continue;
        }

        if !line.ends_with(';') {
            return Err(anyhow!("config parse error on line {line_no}: missing ';'"));
        }

        let line = line.trim_end_matches(';').trim();
        let mut parts = line.splitn(2, char::is_whitespace);

        let key = parts
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("config parse error on line {line_no}: missing key"))?;

        if !allowed.contains(key) {
            return Err(anyhow!(
                "config parse error on line {line_no}: unknown directive '{key}'"
            ));
        }

        let value = parts
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("config parse error on line {line_no}: missing value"))?;

        if key == "model_price" {
            continue;
        }

        if map.contains_key(key) {
            return Err(anyhow!(
                "config parse error on line {line_no}: duplicate directive '{key}'"
            ));
        }

        map.insert(key.to_string(), strip_quotes(value));
    }

    Ok(map)
}

fn strip_quotes(s: &str) -> String {
    let s = s.trim();
    let bytes = s.as_bytes();

    if bytes.len() >= 2 {
        let first = bytes.first().copied();
        let last = bytes.last().copied();

        let double_quoted = first == Some(b'"') && last == Some(b'"');
        let single_quoted = first == Some(b'\'') && last == Some(b'\'');

        if double_quoted || single_quoted {
            return s[1..s.len() - 1].to_string();
        }
    }

    s.to_string()
}

fn get_required(map: &HashMap<String, String>, key: &str) -> Result<String> {
    map.get(key)
        .cloned()
        .ok_or_else(|| cfg_err(format!("missing required config key: {}", key)))
}

fn get_or_default(map: &HashMap<String, String>, key: &str, default: &str) -> String {
    map.get(key).cloned().unwrap_or_else(|| default.to_string())
}

fn parse_or_default<T>(map: &HashMap<String, String>, key: &str, default: T) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match map.get(key) {
        Some(v) => v
            .parse::<T>()
            .map_err(|e| cfg_err(format!("invalid value for {}: {}", key, e))),
        None => Ok(default),
    }
}

#[cfg(test)]
mod tests;
