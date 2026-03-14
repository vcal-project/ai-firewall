use anyhow::{anyhow, Context, Result};
use std::{
    collections::{HashMap, HashSet},
    env, fmt, fs,
    path::Path,
};

#[derive(Clone, Debug)]
pub struct ModelPrice {
    pub input_usd_per_1m_tokens: f64,
    pub output_usd_per_1m_tokens: f64,
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

    pub qdrant_url: String,
    pub qdrant_api_key: Option<String>,
    pub qdrant_collection: String,
    pub qdrant_vector_size: u64,

    pub cache_ttl_seconds: usize,
    pub request_timeout_seconds: u64,

    pub semantic_cache_enabled: bool,
    pub semantic_similarity_threshold: f32,

    pub model_prices: HashMap<String, ModelPrice>,
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.listen_addr.parse::<std::net::SocketAddr>().is_err() {
            return Err(anyhow!("invalid listen_addr: {}", self.listen_addr));
        }

        if self.redis_url.trim().is_empty() {
            return Err(anyhow!("redis_url must not be empty"));
        }

        if !self.redis_url.starts_with("redis://") {
            return Err(anyhow!("redis_url must start with redis://"));
        }

        if self.upstream_base_url.trim().is_empty() {
            return Err(anyhow!("upstream_base_url must not be empty"));
        }

        if self.upstream_api_key.trim().is_empty() {
            return Err(anyhow!("upstream_api_key must not be empty"));
        }

        if self.request_timeout_seconds == 0 {
            return Err(anyhow!("request_timeout_seconds must be > 0"));
        }

        if self.cache_ttl_seconds == 0 {
            return Err(anyhow!("cache_ttl_seconds must be > 0"));
        }

        if !(0.0..=1.0).contains(&self.semantic_similarity_threshold) {
            return Err(anyhow!(
                "semantic_similarity_threshold must be between 0.0 and 1.0"
            ));
        }

        if self.qdrant_vector_size == 0 {
            return Err(anyhow!("qdrant_vector_size must be > 0"));
        }

        if self.semantic_cache_enabled {
            if self.embedding_base_url.trim().is_empty() {
                return Err(anyhow!(
                    "embedding_base_url required when semantic_cache_enabled=true"
                ));
            }

            if self.embedding_model.trim().is_empty() {
                return Err(anyhow!(
                    "embedding_model required when semantic_cache_enabled=true"
                ));
            }

            if self.embedding_api_key.trim().is_empty() {
                return Err(anyhow!(
                    "embedding_api_key required when semantic_cache_enabled=true"
                ));
            }

            if self.qdrant_url.trim().is_empty() {
                return Err(anyhow!(
                    "qdrant_url required when semantic_cache_enabled=true"
                ));
            }
        }

        for (model, price) in &self.model_prices {
            if model.trim().is_empty() {
                return Err(anyhow!("model_prices contains an empty model name"));
            }

            if !price.input_usd_per_1m_tokens.is_finite()
                || !price.output_usd_per_1m_tokens.is_finite()
            {
                return Err(anyhow!(
                    "model_price for '{}' must be finite for both input and output",
                    model
                ));
            }

            if price.input_usd_per_1m_tokens < 0.0 || price.output_usd_per_1m_tokens < 0.0 {
                return Err(anyhow!(
                    "model_price for '{}' must be >= 0 for both input and output",
                    model
                ));
            }
        }

        Ok(())
    }

    pub fn from_env() -> Result<Self> {
        Ok(Self {
            listen_addr: env::var("AIF_LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            redis_url: env::var("AIF_REDIS_URL").context("AIF_REDIS_URL is required")?,

            upstream_base_url: env::var("AIF_UPSTREAM_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".into()),
            upstream_api_key: env::var("AIF_UPSTREAM_API_KEY")
                .context("AIF_UPSTREAM_API_KEY is required")?,

            embedding_base_url: env::var("AIF_EMBEDDING_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".into()),
            embedding_api_key: env::var("AIF_EMBEDDING_API_KEY")
                .unwrap_or_else(|_| env::var("AIF_UPSTREAM_API_KEY").unwrap_or_default()),
            embedding_model: env::var("AIF_EMBEDDING_MODEL")
                .unwrap_or_else(|_| "text-embedding-3-small".into()),

            qdrant_url: env::var("AIF_QDRANT_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:6334".into()),
            qdrant_api_key: env::var("AIF_QDRANT_API_KEY").ok(),
            qdrant_collection: env::var("AIF_QDRANT_COLLECTION")
                .unwrap_or_else(|_| "aif_semantic_cache".into()),
            qdrant_vector_size: env::var("AIF_QDRANT_VECTOR_SIZE")
                .unwrap_or_else(|_| "1536".into())
                .parse()
                .context("invalid AIF_QDRANT_VECTOR_SIZE")?,

            cache_ttl_seconds: env::var("AIF_CACHE_TTL_SECONDS")
                .unwrap_or_else(|_| "86400".into())
                .parse()
                .context("invalid AIF_CACHE_TTL_SECONDS")?,

            request_timeout_seconds: env::var("AIF_REQUEST_TIMEOUT_SECONDS")
                .unwrap_or_else(|_| "120".into())
                .parse()
                .context("invalid AIF_REQUEST_TIMEOUT_SECONDS")?,

            semantic_cache_enabled: env::var("AIF_SEMANTIC_CACHE_ENABLED")
                .unwrap_or_else(|_| "false".into())
                .parse()
                .context("invalid AIF_SEMANTIC_CACHE_ENABLED")?,

            semantic_similarity_threshold: env::var("AIF_SEMANTIC_SIMILARITY_THRESHOLD")
                .unwrap_or_else(|_| "0.92".into())
                .parse()
                .context("invalid AIF_SEMANTIC_SIMILARITY_THRESHOLD")?,

            model_prices: HashMap::new(),
        })
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file {}", path.as_ref().display()))?;

        let map = parse_nginx_style_config(&text)?;
        let model_prices = parse_model_prices(&text)?;

        Ok(Self {
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
        })
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
        "qdrant_url",
        "qdrant_api_key",
        "qdrant_collection",
        "qdrant_vector_size",
        "cache_ttl_seconds",
        "request_timeout_seconds",
        "semantic_cache_enabled",
        "semantic_similarity_threshold",
        "model_price",
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
        .ok_or_else(|| anyhow!("missing required config key: {key}"))
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
            .map_err(|e| anyhow!("invalid value for {key}: {e}")),
        None => Ok(default),
    }
}
