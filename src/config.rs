use crate::release;
use anyhow::{anyhow, Context, Result};
use reqwest::Url;
use std::{
    collections::{HashMap, HashSet},
    env, fmt, fs,
    path::Path,
};

fn cfg_err(msg: impl Into<String>) -> anyhow::Error {
    anyhow!("configuration error: {}", msg.into())
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ProviderKind {
    #[default]
    OpenAiCompatible,
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "openai_compatible",
        }
    }
}

impl std::str::FromStr for ProviderKind {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openai_compatible" | "openai-compatible" | "openai" => Ok(Self::OpenAiCompatible),
            other => Err(format!(
                "unsupported provider '{}'. Supported providers: openai_compatible",
                other
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrivacyGuardMode {
    #[default]
    DetectOnly,
    Redact,
    Anonymize,
    Block,
}

impl PrivacyGuardMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DetectOnly => "detect_only",
            Self::Redact => "redact",
            Self::Anonymize => "anonymize",
            Self::Block => "block",
        }
    }
}

impl std::str::FromStr for PrivacyGuardMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "detect_only" | "detect-only" | "detectonly" => Ok(Self::DetectOnly),
            "redact" => Ok(Self::Redact),
            "anonymize" | "anonymise" => Ok(Self::Anonymize),
            "block" => Ok(Self::Block),
            other => Err(format!(
                "unsupported privacy_guard_mode '{}'. Supported modes: detect_only, redact, anonymize, block",
                other
            )),
        }
    }
}

#[derive(Clone)]
pub struct Config {
    pub listen_addr: String,
    pub redis_url: String,

    pub upstream_provider: ProviderKind,
    pub upstream_base_url: String,
    pub upstream_api_key: String,

    pub embedding_provider: ProviderKind,
    pub embedding_base_url: String,
    pub embedding_api_key: String,
    pub embedding_model: String,
    pub embedding_price: Option<EmbeddingPrice>,

    pub qdrant_url: String,
    pub qdrant_api_key: Option<String>,
    pub qdrant_collection: String,
    pub qdrant_vector_size: u64,

    pub cache_ttl_seconds: usize,
    pub exact_cache_ttl_seconds: usize,
    pub semantic_cache_retention_seconds: usize,
    pub request_timeout_seconds: u64,
    pub upstream_timeout_seconds: u64,
    pub embedding_timeout_seconds: u64,
    pub graceful_shutdown_timeout_seconds: u64,
    pub max_request_body_bytes: usize,
    pub max_prompt_chars: usize,

    pub exact_cache_enabled: bool,
    pub exact_cache_fail_open: bool,
    pub exact_cache_store_enabled: bool,

    pub semantic_cache_enabled: bool,
    pub semantic_similarity_threshold: f32,
    pub semantic_cache_fail_open: bool,
    pub semantic_cache_store_enabled: bool,

    pub privacy_guard_enabled: bool,
    pub privacy_guard_url: String,
    pub privacy_guard_api_key: Option<String>,
    pub privacy_guard_mode: PrivacyGuardMode,
    pub privacy_guard_restore_enabled: bool,
    pub privacy_guard_tenant_id: Option<String>,
    pub privacy_guard_policy_id: Option<String>,
    pub privacy_guard_timeout_seconds: u64,
    pub guard_fail_open: bool,

    pub cache_bypass_header: String,
    pub metrics_auth_required: bool,
    pub metrics_auth_token: Option<String>,

    pub readiness_requires_redis: bool,
    pub readiness_requires_qdrant: bool,
    pub readiness_requires_upstream: bool,

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
        if let Some(err) =
            validate_openai_compatible_base_url("upstream_base_url", &self.upstream_base_url)
        {
            errors.push(err);
        }

        if self.upstream_api_key.trim().is_empty() {
            errors.push(
                "upstream_api_key must not be empty. For local providers without authentication, use dummy, none, null, or -"
                    .into(),
            );
        }

        // ---- upstream provider
        match self.upstream_provider {
            ProviderKind::OpenAiCompatible => {
                // Currently all upstream providers are OpenAI-compatible HTTP endpoints.
            }
        }

        // ---- timeouts
        if self.request_timeout_seconds == 0 {
            errors.push("request_timeout_seconds must be > 0".into());
        }

        if self.upstream_timeout_seconds == 0 {
            errors.push("upstream_timeout_seconds must be > 0".into());
        }

        if self.embedding_timeout_seconds == 0 {
            errors.push("embedding_timeout_seconds must be > 0".into());
        }

        if self.cache_ttl_seconds == 0 {
            errors.push("cache_ttl_seconds must be > 0".into());
        }

        if self.exact_cache_ttl_seconds == 0 {
            errors.push("exact_cache_ttl_seconds must be > 0".into());
        }

        if self.semantic_cache_enabled && self.semantic_cache_retention_seconds == 0 {
            errors.push(
                "semantic_cache_retention_seconds must be > 0 when semantic_cache_enabled=true"
                    .into(),
            );
        }

        if self.graceful_shutdown_timeout_seconds == 0 {
            errors.push("graceful_shutdown_timeout_seconds must be > 0".into());
        }

        // ---- request size
        if self.max_request_body_bytes == 0 {
            errors.push("max_request_body_bytes must be > 0 (example: 1M, 512K, 1048576)".into());
        }

        if self.max_prompt_chars == 0 {
            errors.push("max_prompt_chars must be > 0".into());
        }

        if self.cache_bypass_header.trim().is_empty() {
            errors.push("cache_bypass_header must not be empty".into());
        }

        if self.privacy_guard_enabled {
            if self.privacy_guard_url.trim().is_empty() {
                errors.push(
                    "privacy_guard_url must not be empty when privacy_guard_enabled=true".into(),
                );
            } else if !looks_like_http_url(&self.privacy_guard_url) {
                errors.push(format!(
                    "invalid privacy_guard_url '{}': must start with http:// or https://",
                    self.privacy_guard_url
                ));
            }

            if self.privacy_guard_timeout_seconds == 0 {
                errors.push(
                    "privacy_guard_timeout_seconds must be > 0 when privacy_guard_enabled=true"
                        .into(),
                );
            }
        }

        if self.metrics_auth_required {
            match self.metrics_auth_token.as_deref() {
                Some(token) if !token.trim().is_empty() => {}
                _ => errors
                    .push("metrics_auth_token must be set when metrics_auth_required=true".into()),
            }
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

            if self.embedding_api_key.trim().is_empty() {
                errors.push(
                    "embedding_api_key must not be empty when semantic_cache_enabled=true. For local embedding providers without authentication, use dummy, none, null, or -"
                        .into(),
                );
            }

            if let Some(err) =
                validate_openai_compatible_base_url("embedding_base_url", &self.embedding_base_url)
            {
                errors.push(err);
            }

            // ---- embedding provider
            match self.embedding_provider {
                ProviderKind::OpenAiCompatible => {
                    // Currently all embedding providers are OpenAI-compatible HTTP endpoints.
                }
            }

            if !self.qdrant_url.trim().is_empty() && !looks_like_http_url(&self.qdrant_url) {
                errors.push(format!(
                    "invalid qdrant_url '{}': must start with http:// or https://",
                    self.qdrant_url
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

    pub fn to_masked_display(&self) -> String {
        fn mask_secret_value(value: &str) -> String {
            let value = value.trim();

            if value.is_empty() {
                return "<empty>".to_string();
            }

            if matches!(
                value.to_ascii_lowercase().as_str(),
                "dummy" | "none" | "null" | "-"
            ) {
                return value.to_string();
            }

            let chars: Vec<char> = value.chars().collect();

            if chars.len() <= 8 {
                return "****".to_string();
            }

            let prefix: String = chars.iter().take(4).collect();
            let suffix: String = chars
                .iter()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();

            format!("{prefix}...{suffix}")
        }

        fn mask_optional_secret(value: &Option<String>) -> String {
            match value {
                Some(v) => mask_secret_value(v),
                None => "<not set>".to_string(),
            }
        }

        let embedding_price = self
            .embedding_price
            .as_ref()
            .map(|p| p.usd_per_1m_tokens.to_string())
            .unwrap_or_else(|| "<not set>".to_string());

        let mut out = String::new();

        out.push_str("AI Cost Firewall configuration\n");
        out.push_str("--------------------------------\n");
        out.push_str(&format!("version = {}\n", release::PRODUCT_VERSION));
        out.push_str(&format!("release = {}\n", release::RELEASE_TITLE));
        out.push_str(&format!(
            "compatibility_model = {}\n",
            release::COMPATIBILITY_MODEL
        ));
        out.push_str(&format!("scope_note = {}\n", release::SCOPE_NOTE));

        out.push_str(&format!("listen_addr = {}\n", self.listen_addr));
        out.push_str(&format!(
            "redis_url = {}\n",
            mask_secret_value(&self.redis_url)
        ));

        out.push_str(&format!(
            "upstream_provider = {}\n",
            self.upstream_provider.as_str()
        ));
        out.push_str(&format!("upstream_base_url = {}\n", self.upstream_base_url));
        out.push_str(&format!(
            "upstream_api_key = {}\n",
            mask_secret_value(&self.upstream_api_key)
        ));

        out.push_str(&format!(
            "request_timeout_seconds = {}\n",
            self.request_timeout_seconds
        ));
        out.push_str(&format!("cache_ttl_seconds = {}\n", self.cache_ttl_seconds));
        out.push_str(&format!(
            "exact_cache_ttl_seconds = {}\n",
            self.exact_cache_ttl_seconds
        ));
        out.push_str(&format!(
            "max_request_body_bytes = {}\n",
            self.max_request_body_bytes
        ));
        out.push_str(&format!(
            "allow_unknown_models_pass_through = {}\n",
            self.allow_unknown_models_pass_through
        ));

        out.push_str("\nSemantic cache\n");
        out.push_str("--------------------------------\n");

        out.push_str(&format!(
            "semantic_cache_enabled = {}\n",
            self.semantic_cache_enabled
        ));
        out.push_str(&format!(
            "semantic_cache_fail_open = {}\n",
            self.semantic_cache_fail_open
        ));
        out.push_str(&format!(
            "semantic_similarity_threshold = {}\n",
            self.semantic_similarity_threshold
        ));
        out.push_str(&format!(
            "semantic_cache_retention_seconds = {}\n",
            self.semantic_cache_retention_seconds
        ));

        out.push_str(&format!(
            "embedding_provider = {}\n",
            self.embedding_provider.as_str()
        ));
        out.push_str(&format!(
            "embedding_base_url = {}\n",
            self.embedding_base_url
        ));
        out.push_str(&format!(
            "embedding_api_key = {}\n",
            mask_secret_value(&self.embedding_api_key)
        ));
        out.push_str(&format!("embedding_model = {}\n", self.embedding_model));
        out.push_str(&format!(
            "embedding_price_usd_per_1m_tokens = {}\n",
            embedding_price
        ));

        out.push_str(&format!("qdrant_url = {}\n", self.qdrant_url));
        out.push_str(&format!(
            "qdrant_api_key = {}\n",
            mask_optional_secret(&self.qdrant_api_key)
        ));
        out.push_str(&format!("qdrant_collection = {}\n", self.qdrant_collection));
        out.push_str(&format!(
            "qdrant_vector_size = {}\n",
            self.qdrant_vector_size
        ));

        out.push_str("\nVCAL Privacy Guard\n");
        out.push_str("--------------------------------\n");
        out.push_str(&format!(
            "privacy_guard_enabled = {}\n",
            self.privacy_guard_enabled
        ));
        out.push_str(&format!("privacy_guard_url = {}\n", self.privacy_guard_url));
        out.push_str(&format!(
            "privacy_guard_api_key = {}\n",
            mask_optional_secret(&self.privacy_guard_api_key)
        ));
        out.push_str(&format!(
            "privacy_guard_mode = {}\n",
            self.privacy_guard_mode.as_str()
        ));
        out.push_str(&format!(
            "privacy_guard_restore_enabled = {}\n",
            self.privacy_guard_restore_enabled
        ));
        out.push_str(&format!(
            "privacy_guard_timeout_seconds = {}\n",
            self.privacy_guard_timeout_seconds
        ));
        out.push_str(&format!("guard_fail_open = {}\n", self.guard_fail_open));

        out.push_str("\nLifecycle\n");
        out.push_str("--------------------------------\n");

        out.push_str(&format!(
            "graceful_shutdown_timeout_seconds = {}\n",
            self.graceful_shutdown_timeout_seconds
        ));

        out.push_str("\nModel prices\n");
        out.push_str("--------------------------------\n");

        if self.model_prices.is_empty() {
            out.push_str("<none>\n");
        } else {
            let mut models: Vec<_> = self.model_prices.iter().collect();
            models.sort_by(|a, b| a.0.cmp(b.0));

            for (model, price) in models {
                out.push_str(&format!(
                    "{}: input_usd_per_1m_tokens = {}, output_usd_per_1m_tokens = {}\n",
                    model, price.input_usd_per_1m_tokens, price.output_usd_per_1m_tokens
                ));
            }
        }

        out
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

        // v0.1.5: keep legacy cache_ttl_seconds as the default for both cache layers.
        let cache_ttl_seconds = parse_or_default(&map, "cache_ttl_seconds", 86400usize)?;

        let exact_cache_ttl_seconds =
            parse_or_default(&map, "exact_cache_ttl_seconds", cache_ttl_seconds)?;

        let semantic_cache_retention_seconds =
            parse_or_default(&map, "semantic_cache_retention_seconds", cache_ttl_seconds)?;

        let request_timeout_seconds = parse_or_default(&map, "request_timeout_seconds", 120u64)?;
        let upstream_timeout_seconds =
            parse_or_default(&map, "upstream_timeout_seconds", request_timeout_seconds)?;
        let embedding_timeout_seconds =
            parse_or_default(&map, "embedding_timeout_seconds", request_timeout_seconds)?;

        let cfg = Self {
            listen_addr: get_or_default(&map, "listen_addr", "0.0.0.0:8080"),
            redis_url: get_required(&map, "redis_url")?,

            upstream_provider: parse_or_default(
                &map,
                "upstream_provider",
                ProviderKind::OpenAiCompatible,
            )?,
            upstream_base_url: get_or_default(&map, "upstream_base_url", "https://api.openai.com"),
            upstream_api_key: get_required(&map, "upstream_api_key")?,

            embedding_provider: parse_or_default(
                &map,
                "embedding_provider",
                ProviderKind::OpenAiCompatible,
            )?,

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

            cache_ttl_seconds,
            exact_cache_ttl_seconds,
            semantic_cache_retention_seconds,

            request_timeout_seconds,
            upstream_timeout_seconds,
            embedding_timeout_seconds,

            exact_cache_enabled: parse_or_default(&map, "exact_cache_enabled", true)?,
            exact_cache_fail_open: parse_or_default(&map, "exact_cache_fail_open", true)?,
            exact_cache_store_enabled: parse_or_default(&map, "exact_cache_store_enabled", true)?,

            semantic_cache_enabled: parse_or_default(&map, "semantic_cache_enabled", false)?,
            semantic_similarity_threshold: parse_or_default(
                &map,
                "semantic_similarity_threshold",
                0.92f32,
            )?,

            semantic_cache_fail_open: parse_or_default(&map, "semantic_cache_fail_open", true)?,
            semantic_cache_store_enabled: parse_or_default(
                &map,
                "semantic_cache_store_enabled",
                true,
            )?,

            privacy_guard_enabled: parse_or_default(&map, "privacy_guard_enabled", false)?,
            privacy_guard_url: get_or_default(&map, "privacy_guard_url", "http://127.0.0.1:8090"),
            privacy_guard_api_key: map.get("privacy_guard_api_key").cloned(),
            privacy_guard_mode: parse_or_default(
                &map,
                "privacy_guard_mode",
                PrivacyGuardMode::DetectOnly,
            )?,
            privacy_guard_restore_enabled: parse_or_default(
                &map,
                "privacy_guard_restore_enabled",
                true,
            )?,
            privacy_guard_tenant_id: map.get("privacy_guard_tenant_id").cloned(),
            privacy_guard_policy_id: map.get("privacy_guard_policy_id").cloned(),
            privacy_guard_timeout_seconds: parse_or_default(
                &map,
                "privacy_guard_timeout_seconds",
                10u64,
            )?,
            guard_fail_open: parse_or_default(&map, "guard_fail_open", true)?,

            cache_bypass_header: get_or_default(&map, "cache_bypass_header", "X-AIF-Cache-Bypass"),
            metrics_auth_required: parse_or_default(&map, "metrics_auth_required", false)?,
            metrics_auth_token: map.get("metrics_auth_token").cloned(),

            readiness_requires_redis: parse_or_default(&map, "readiness_requires_redis", true)?,
            readiness_requires_qdrant: parse_or_default(&map, "readiness_requires_qdrant", false)?,
            readiness_requires_upstream: parse_or_default(
                &map,
                "readiness_requires_upstream",
                false,
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
            max_prompt_chars: parse_or_default(&map, "max_prompt_chars", 200_000usize)?,
        };

        let cache_ttl_was_set = map.contains_key("cache_ttl_seconds");
        let exact_ttl_was_set = map.contains_key("exact_cache_ttl_seconds");
        let semantic_ttl_was_set = map.contains_key("semantic_cache_retention_seconds");

        if cache_ttl_was_set && (exact_ttl_was_set || semantic_ttl_was_set) {
            tracing::warn!(
                cache_ttl_seconds = cfg.cache_ttl_seconds,
                exact_cache_ttl_seconds = cfg.exact_cache_ttl_seconds,
                semantic_cache_retention_seconds = cfg.semantic_cache_retention_seconds,
                "cache_ttl_seconds is configured together with explicit TTLs; explicit TTLs take precedence"
            );
        }

        warn_if_suspicious(&cfg);

        Ok(cfg)
    }

    pub fn from_env() -> Result<Self> {
        // v0.1.5: legacy cache_ttl_seconds remains the default for both cache layers.
        let cache_ttl_seconds: usize = {
            let raw = env::var("AIF_CACHE_TTL_SECONDS").unwrap_or_else(|_| "86400".into());

            raw.parse::<usize>().map_err(|e| {
                cfg_err(format!(
                    "invalid AIF_CACHE_TTL_SECONDS value '{}': {}",
                    raw, e
                ))
            })?
        };

        let exact_cache_ttl_seconds = {
            let raw = env::var("AIF_EXACT_CACHE_TTL_SECONDS")
                .unwrap_or_else(|_| cache_ttl_seconds.to_string());
            raw.parse().map_err(|e| {
                cfg_err(format!(
                    "invalid AIF_EXACT_CACHE_TTL_SECONDS value '{}': {}",
                    raw, e
                ))
            })?
        };

        let semantic_cache_retention_seconds = {
            let raw = env::var("AIF_SEMANTIC_CACHE_RETENTION_SECONDS")
                .unwrap_or_else(|_| cache_ttl_seconds.to_string());
            raw.parse().map_err(|e| {
                cfg_err(format!(
                    "invalid AIF_SEMANTIC_CACHE_RETENTION_SECONDS value '{}': {}",
                    raw, e
                ))
            })?
        };

        let cfg = Self {
            listen_addr: env::var("AIF_LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            redis_url: env::var("AIF_REDIS_URL")
                .map_err(|_| cfg_err("AIF_REDIS_URL is required when no config file is used"))?,

            upstream_provider: {
                let raw = env::var("AIF_UPSTREAM_PROVIDER")
                    .unwrap_or_else(|_| "openai_compatible".into());
                raw.parse::<ProviderKind>().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_UPSTREAM_PROVIDER value '{}': {}",
                        raw, e
                    ))
                })?
            },

            upstream_base_url: env::var("AIF_UPSTREAM_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".into()),
            upstream_api_key: env::var("AIF_UPSTREAM_API_KEY").map_err(|_| {
                cfg_err("AIF_UPSTREAM_API_KEY is required when no config file is used")
            })?,

            embedding_provider: {
                let raw = env::var("AIF_EMBEDDING_PROVIDER")
                    .unwrap_or_else(|_| "openai_compatible".into());
                raw.parse::<ProviderKind>().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_EMBEDDING_PROVIDER value '{}': {}",
                        raw, e
                    ))
                })?
            },

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

            cache_ttl_seconds,
            exact_cache_ttl_seconds,
            semantic_cache_retention_seconds,

            request_timeout_seconds: {
                let raw = env::var("AIF_REQUEST_TIMEOUT_SECONDS").unwrap_or_else(|_| "120".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_REQUEST_TIMEOUT_SECONDS value '{}': {}",
                        raw, e
                    ))
                })?
            },
            upstream_timeout_seconds: {
                let default =
                    env::var("AIF_REQUEST_TIMEOUT_SECONDS").unwrap_or_else(|_| "120".into());
                let raw = env::var("AIF_UPSTREAM_TIMEOUT_SECONDS").unwrap_or(default);
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_UPSTREAM_TIMEOUT_SECONDS value '{}': {}",
                        raw, e
                    ))
                })?
            },
            embedding_timeout_seconds: {
                let default =
                    env::var("AIF_REQUEST_TIMEOUT_SECONDS").unwrap_or_else(|_| "120".into());
                let raw = env::var("AIF_EMBEDDING_TIMEOUT_SECONDS").unwrap_or(default);
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_EMBEDDING_TIMEOUT_SECONDS value '{}': {}",
                        raw, e
                    ))
                })?
            },

            exact_cache_enabled: {
                let raw = env::var("AIF_EXACT_CACHE_ENABLED").unwrap_or_else(|_| "true".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_EXACT_CACHE_ENABLED value '{}': {}",
                        raw, e
                    ))
                })?
            },
            exact_cache_fail_open: {
                let raw = env::var("AIF_EXACT_CACHE_FAIL_OPEN").unwrap_or_else(|_| "true".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_EXACT_CACHE_FAIL_OPEN value '{}': {}",
                        raw, e
                    ))
                })?
            },
            exact_cache_store_enabled: {
                let raw =
                    env::var("AIF_EXACT_CACHE_STORE_ENABLED").unwrap_or_else(|_| "true".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_EXACT_CACHE_STORE_ENABLED value '{}': {}",
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

            semantic_cache_fail_open: {
                let raw =
                    env::var("AIF_SEMANTIC_CACHE_FAIL_OPEN").unwrap_or_else(|_| "true".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_SEMANTIC_CACHE_FAIL_OPEN value '{}': {}",
                        raw, e
                    ))
                })?
            },
            semantic_cache_store_enabled: {
                let raw =
                    env::var("AIF_SEMANTIC_CACHE_STORE_ENABLED").unwrap_or_else(|_| "true".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_SEMANTIC_CACHE_STORE_ENABLED value '{}': {}",
                        raw, e
                    ))
                })?
            },

            privacy_guard_enabled: {
                let raw = env::var("AIF_PRIVACY_GUARD_ENABLED").unwrap_or_else(|_| "false".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_PRIVACY_GUARD_ENABLED value '{}': {}",
                        raw, e
                    ))
                })?
            },
            privacy_guard_url: env::var("AIF_PRIVACY_GUARD_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8090".into()),
            privacy_guard_api_key: env::var("AIF_PRIVACY_GUARD_API_KEY").ok(),
            privacy_guard_mode: {
                let raw =
                    env::var("AIF_PRIVACY_GUARD_MODE").unwrap_or_else(|_| "detect_only".into());
                raw.parse::<PrivacyGuardMode>().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_PRIVACY_GUARD_MODE value '{}': {}",
                        raw, e
                    ))
                })?
            },
            privacy_guard_restore_enabled: {
                let raw =
                    env::var("AIF_PRIVACY_GUARD_RESTORE_ENABLED").unwrap_or_else(|_| "true".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_PRIVACY_GUARD_RESTORE_ENABLED value '{}': {}",
                        raw, e
                    ))
                })?
            },
            privacy_guard_tenant_id: env::var("AIF_PRIVACY_GUARD_TENANT_ID").ok(),
            privacy_guard_policy_id: env::var("AIF_PRIVACY_GUARD_POLICY_ID").ok(),
            privacy_guard_timeout_seconds: {
                let raw =
                    env::var("AIF_PRIVACY_GUARD_TIMEOUT_SECONDS").unwrap_or_else(|_| "10".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_PRIVACY_GUARD_TIMEOUT_SECONDS value '{}': {}",
                        raw, e
                    ))
                })?
            },
            guard_fail_open: {
                let raw = env::var("AIF_GUARD_FAIL_OPEN").unwrap_or_else(|_| "true".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_GUARD_FAIL_OPEN value '{}': {}",
                        raw, e
                    ))
                })?
            },

            cache_bypass_header: env::var("AIF_CACHE_BYPASS_HEADER")
                .unwrap_or_else(|_| "X-AIF-Cache-Bypass".into()),
            metrics_auth_required: {
                let raw = env::var("AIF_METRICS_AUTH_REQUIRED").unwrap_or_else(|_| "false".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_METRICS_AUTH_REQUIRED value '{}': {}",
                        raw, e
                    ))
                })?
            },
            metrics_auth_token: env::var("AIF_METRICS_AUTH_TOKEN").ok(),
            readiness_requires_redis: {
                let raw =
                    env::var("AIF_READINESS_REQUIRES_REDIS").unwrap_or_else(|_| "true".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_READINESS_REQUIRES_REDIS value '{}': {}",
                        raw, e
                    ))
                })?
            },
            readiness_requires_qdrant: {
                let raw =
                    env::var("AIF_READINESS_REQUIRES_QDRANT").unwrap_or_else(|_| "false".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_READINESS_REQUIRES_QDRANT value '{}': {}",
                        raw, e
                    ))
                })?
            },
            readiness_requires_upstream: {
                let raw =
                    env::var("AIF_READINESS_REQUIRES_UPSTREAM").unwrap_or_else(|_| "false".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_READINESS_REQUIRES_UPSTREAM value '{}': {}",
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
            max_prompt_chars: {
                let raw = env::var("AIF_MAX_PROMPT_CHARS").unwrap_or_else(|_| "200000".into());
                raw.parse().map_err(|e| {
                    cfg_err(format!(
                        "invalid AIF_MAX_PROMPT_CHARS value '{}': {}",
                        raw, e
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

    pub fn semantic_cache_status(&self) -> &'static str {
        if self.semantic_cache_enabled {
            "enabled"
        } else {
            "disabled"
        }
    }

    pub fn startup_summary_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("- upstream provider: {}", self.upstream_provider.as_str()),
            format!("- upstream base URL: {}", self.upstream_base_url),
            format!("- exact cache enabled: {}", self.exact_cache_enabled),
            format!("- semantic cache: {}", self.semantic_cache_status()),
            format!(
                "- request timeout fallback: {}s",
                self.request_timeout_seconds
            ),
            format!("- upstream timeout: {}s", self.upstream_timeout_seconds),
            format!("- embedding timeout: {}s", self.embedding_timeout_seconds),
            format!("- max request body: {} bytes", self.max_request_body_bytes),
            format!("- max prompt chars: {}", self.max_prompt_chars),
            format!("- exact cache TTL: {}s", self.exact_cache_ttl_seconds),
            format!("- exact fail-open: {}", self.exact_cache_fail_open),
            format!("- cache bypass header: {}", self.cache_bypass_header),
            format!("- privacy guard enabled: {}", self.privacy_guard_enabled),
            format!("- guard fail-open: {}", self.guard_fail_open),
        ];

        if self.semantic_cache_enabled {
            lines.push(format!(
                "- embedding provider: {}",
                self.embedding_provider.as_str()
            ));
            lines.push(format!("- embedding base URL: {}", self.embedding_base_url));
            lines.push(format!("- embedding model: {}", self.embedding_model));
            lines.push(format!("- qdrant URL: {}", self.qdrant_url));
            lines.push(format!("- qdrant collection: {}", self.qdrant_collection));
            lines.push(format!("- qdrant vector size: {}", self.qdrant_vector_size));
            lines.push(format!(
                "- semantic similarity threshold: {}",
                self.semantic_similarity_threshold
            ));
            lines.push(format!(
                "- semantic retention: {}s",
                self.semantic_cache_retention_seconds
            ));
            lines.push(format!(
                "- semantic fail-open: {}",
                self.semantic_cache_fail_open
            ));
        }

        lines
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("listen_addr", &self.listen_addr)
            .field("redis_url", &self.redis_url)
            .field("upstream_provider", &self.upstream_provider.as_str())
            .field("upstream_base_url", &self.upstream_base_url)
            .field("upstream_api_key", &mask_secret(&self.upstream_api_key))
            .field("embedding_provider", &self.embedding_provider.as_str())
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
            .field("exact_cache_ttl_seconds", &self.exact_cache_ttl_seconds)
            .field(
                "semantic_cache_retention_seconds",
                &self.semantic_cache_retention_seconds,
            )
            .field("request_timeout_seconds", &self.request_timeout_seconds)
            .field("upstream_timeout_seconds", &self.upstream_timeout_seconds)
            .field("embedding_timeout_seconds", &self.embedding_timeout_seconds)
            .field("max_prompt_chars", &self.max_prompt_chars)
            .field("exact_cache_enabled", &self.exact_cache_enabled)
            .field("exact_cache_fail_open", &self.exact_cache_fail_open)
            .field("exact_cache_store_enabled", &self.exact_cache_store_enabled)
            .field("semantic_cache_enabled", &self.semantic_cache_enabled)
            .field(
                "semantic_similarity_threshold",
                &self.semantic_similarity_threshold,
            )
            .field("semantic_cache_fail_open", &self.semantic_cache_fail_open)
            .field(
                "semantic_cache_store_enabled",
                &self.semantic_cache_store_enabled,
            )
            .field("privacy_guard_enabled", &self.privacy_guard_enabled)
            .field("privacy_guard_url", &self.privacy_guard_url)
            .field(
                "privacy_guard_api_key",
                &self.privacy_guard_api_key.as_ref().map(|k| mask_secret(k)),
            )
            .field("privacy_guard_mode", &self.privacy_guard_mode.as_str())
            .field(
                "privacy_guard_restore_enabled",
                &self.privacy_guard_restore_enabled,
            )
            .field("privacy_guard_tenant_id", &self.privacy_guard_tenant_id)
            .field("privacy_guard_policy_id", &self.privacy_guard_policy_id)
            .field(
                "privacy_guard_timeout_seconds",
                &self.privacy_guard_timeout_seconds,
            )
            .field("guard_fail_open", &self.guard_fail_open)
            .field("cache_bypass_header", &self.cache_bypass_header)
            .field("metrics_auth_required", &self.metrics_auth_required)
            .field(
                "metrics_auth_token",
                &self.metrics_auth_token.as_ref().map(|k| mask_secret(k)),
            )
            .field("readiness_requires_redis", &self.readiness_requires_redis)
            .field("readiness_requires_qdrant", &self.readiness_requires_qdrant)
            .field(
                "readiness_requires_upstream",
                &self.readiness_requires_upstream,
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
        "upstream_provider",
        "upstream_base_url",
        "upstream_api_key",
        "embedding_provider",
        "embedding_base_url",
        "embedding_api_key",
        "embedding_model",
        "embedding_price",
        "qdrant_url",
        "qdrant_api_key",
        "qdrant_collection",
        "qdrant_vector_size",
        "cache_ttl_seconds",
        "exact_cache_ttl_seconds",
        "semantic_cache_retention_seconds",
        "request_timeout_seconds",
        "upstream_timeout_seconds",
        "embedding_timeout_seconds",
        "exact_cache_enabled",
        "exact_cache_fail_open",
        "exact_cache_store_enabled",
        "semantic_cache_enabled",
        "semantic_similarity_threshold",
        "semantic_cache_fail_open",
        "semantic_cache_store_enabled",
        "privacy_guard_enabled",
        "privacy_guard_url",
        "privacy_guard_api_key",
        "privacy_guard_mode",
        "privacy_guard_restore_enabled",
        "privacy_guard_tenant_id",
        "privacy_guard_policy_id",
        "privacy_guard_timeout_seconds",
        "guard_fail_open",
        "cache_bypass_header",
        "metrics_auth_required",
        "metrics_auth_token",
        "readiness_requires_redis",
        "readiness_requires_qdrant",
        "readiness_requires_upstream",
        "model_price",
        "allow_unknown_models_pass_through",
        "graceful_shutdown_timeout_seconds",
        "max_request_body_bytes",
        "max_prompt_chars",
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

fn looks_like_http_url(s: &str) -> bool {
    let s = s.trim();
    s.starts_with("http://") || s.starts_with("https://")
}

fn validate_openai_compatible_base_url(name: &str, value: &str) -> Option<String> {
    let raw = value.trim();

    if raw.is_empty() {
        return Some(format!("{name} must not be empty"));
    }

    let parsed = match Url::parse(raw) {
        Ok(url) => url,
        Err(e) => {
            return Some(format!(
                "invalid {name} '{}': expected a full http:// or https:// base URL: {}",
                value, e
            ));
        }
    };

    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Some(format!(
                "invalid {name} '{}': unsupported scheme '{}'; use http:// or https://",
                value, other
            ));
        }
    }

    if parsed.host_str().is_none() {
        return Some(format!(
            "invalid {name} '{}': URL must include a hostname",
            value
        ));
    }

    let path = parsed.path().trim_end_matches('/').to_ascii_lowercase();

    if path.ends_with("/chat/completions") || path.ends_with("/embeddings") {
        return Some(format!(
            "invalid {name} '{}': configure a base URL, not a full endpoint path. Use the provider root URL or its /v1 base path.",
            value
        ));
    }

    if !path.is_empty() && path != "/" && path != "/v1" && !path.ends_with("/v1") {
        return Some(format!(
            "invalid {name} '{}': unsupported OpenAI-compatible base path '{}'. Use the provider root URL or its /v1 base path.",
            value,
            parsed.path()
        ));
    }

    None
}

fn warn_if_suspicious(cfg: &Config) {
    if cfg.max_request_body_bytes < 1024 {
        tracing::warn!(
            "max_request_body_bytes={} is very small; requests larger than this will be rejected. Consider using at least 1K, for example 512K or 1M",
            cfg.max_request_body_bytes
        );
    }

    if cfg.graceful_shutdown_timeout_seconds <= 1 {
        tracing::warn!(
            "graceful_shutdown_timeout_seconds={} is very small; in-flight requests may not have enough time to drain cleanly",
            cfg.graceful_shutdown_timeout_seconds
        );
    }

    if cfg.privacy_guard_enabled {
        tracing::info!(
            privacy_guard_url = cfg.privacy_guard_url,
            privacy_guard_mode = cfg.privacy_guard_mode.as_str(),
            privacy_guard_restore_enabled = cfg.privacy_guard_restore_enabled,
            guard_fail_open = cfg.guard_fail_open,
            "VCAL Privacy Guard orchestration is enabled"
        );
    }

    if cfg.semantic_cache_fail_open && !cfg.semantic_cache_enabled {
        tracing::info!(
            "semantic_cache_fail_open=true has no effect because semantic_cache_enabled=false"
        );
    }

    if cfg.semantic_cache_enabled && cfg.semantic_cache_fail_open {
        tracing::info!(
            "semantic_cache_fail_open=true; runtime semantic lookup failures will skip semantic cache and continue upstream. Startup still requires semantic dependencies to initialize."
        );
    }

    if cfg.semantic_cache_enabled && cfg.embedding_provider == ProviderKind::OpenAiCompatible {
        tracing::info!(
            embedding_base_url = cfg.embedding_base_url,
            embedding_model = cfg.embedding_model,
            qdrant_vector_size = cfg.qdrant_vector_size,
            "using OpenAI-compatible embedding provider; ensure qdrant_vector_size matches the embedding model dimension"
        );
    }

    if cfg.semantic_cache_enabled && cfg.embedding_price.is_none() {
        tracing::warn!(
            "semantic_cache_enabled=true but embedding_price is not configured; net savings metrics may be incomplete"
        );
    }

    if cfg.semantic_cache_enabled && cfg.semantic_cache_retention_seconds > 7 * 24 * 3600 {
        tracing::warn!(
            semantic_cache_retention_seconds = cfg.semantic_cache_retention_seconds,
            "semantic_cache_retention_seconds is relatively long; this may increase Qdrant storage usage. Consider periodic cleanup with --prune-expired-semantic-cache"
        );
    }

    if cfg.semantic_cache_enabled
        && cfg.semantic_cache_retention_seconds == cfg.exact_cache_ttl_seconds
    {
        tracing::info!(
            semantic_cache_retention_seconds = cfg.semantic_cache_retention_seconds,
            exact_cache_ttl_seconds = cfg.exact_cache_ttl_seconds,
            "semantic and exact cache TTLs are equal; consider longer semantic retention to improve reuse"
        );
    }
}

#[cfg(test)]
mod tests;
