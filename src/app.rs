use crate::{core::pricing::is_priced_model, release};

use crate::{
    api,
    cache::{exact::ExactCache, noop_exact::NoopExactCache, redis_exact::RedisExactCache},
    config::{Config, ProviderKind},
    embeddings::{openai::OpenAiEmbeddingProvider, provider::EmbeddingProvider},
    evidence::{
        buffered_http::{
            BufferedHttpEvidenceHandle, BufferedHttpEvidenceSettings, BufferedHttpEvidenceSink,
        },
        EvidenceSink, TracingEvidenceSink,
    },
    guards::build_guard_orchestrator,
    metrics,
    semantic::{
        noop::NoopSemanticCache, qdrant::QdrantSemanticCache, semantic_cache::SemanticCache,
    },
    services::chat_service::{ChatService, ChatServiceDeps, ChatServiceSettings, UpstreamMetadata},
    upstream::build_llm_upstream,
};

use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderMap, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use redis::aio::ConnectionManager;
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{Mutex, RwLock};
use tower_http::trace::TraceLayer;

#[derive(Clone, Debug)]
pub struct ShutdownState {
    pub ready: Arc<AtomicBool>,
    pub shutting_down: Arc<AtomicBool>,
    pub inflight_requests: Arc<AtomicU64>,
}

impl ShutdownState {
    pub fn new() -> Self {
        metrics::READINESS_STATE.set(1);
        metrics::SHUTDOWN_IN_PROGRESS.set(0);

        Self {
            ready: Arc::new(AtomicBool::new(true)),
            shutting_down: Arc::new(AtomicBool::new(false)),
            inflight_requests: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn begin_shutdown(&self) {
        self.ready.store(false, Ordering::SeqCst);
        self.shutting_down.store(true, Ordering::SeqCst);

        metrics::READINESS_STATE.set(0);
        metrics::SHUTDOWN_IN_PROGRESS.set(1);

        tracing::info!("graceful shutdown started; readiness disabled");
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Relaxed)
    }

    pub fn inflight(&self) -> u64 {
        self.inflight_requests.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Debug)]
pub struct DependencyState {
    pub redis_available: Arc<AtomicBool>,
    pub qdrant_available: Arc<AtomicBool>,
    pub upstream_available: Arc<AtomicBool>,
}

impl DependencyState {
    pub fn new(redis_available: bool, qdrant_available: bool, upstream_available: bool) -> Self {
        Self {
            redis_available: Arc::new(AtomicBool::new(redis_available)),
            qdrant_available: Arc::new(AtomicBool::new(qdrant_available)),
            upstream_available: Arc::new(AtomicBool::new(upstream_available)),
        }
    }

    pub fn update(&self, other: &Self) {
        self.redis_available.store(
            other.redis_available.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.qdrant_available.store(
            other.qdrant_available.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.upstream_available.store(
            other.upstream_available.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<Config>>,
    pub chat_service: Arc<RwLock<Arc<ChatService>>>,
    pub dependencies: DependencyState,
    pub evidence_delivery: Arc<Mutex<Option<BufferedHttpEvidenceHandle>>>,
    pub shutdown: ShutdownState,
}

impl AppState {
    pub async fn chat_service(&self) -> Arc<ChatService> {
        let guard = self.chat_service.read().await;
        guard.clone()
    }

    pub async fn allow_unknown_models_pass_through(&self) -> bool {
        let cfg = self.config.read().await;
        cfg.allow_unknown_models_pass_through
    }

    pub async fn replace_evidence_delivery(
        &self,
        replacement: Option<BufferedHttpEvidenceHandle>,
        flush_timeout: Duration,
    ) {
        let previous = {
            let mut guard = self.evidence_delivery.lock().await;
            std::mem::replace(&mut *guard, replacement)
        };

        if let Some(handle) = previous {
            if let Err(error) = handle.shutdown(flush_timeout).await {
                tracing::warn!(error = %error, "failed to flush previous VCAL Audit worker");
            }
        }
    }

    pub async fn shutdown_evidence_delivery(&self, flush_timeout: Duration) {
        self.replace_evidence_delivery(None, flush_timeout).await;
    }

    pub async fn is_model_allowed(&self, model: &str) -> bool {
        let cfg = self.config.read().await;

        if model.is_empty() {
            return false;
        }

        is_priced_model(model, &cfg.model_prices)
    }
}

pub struct BuiltApp {
    pub router: Router,
    pub state: Arc<AppState>,
}

fn infer_upstream_provider_name(provider_type: &str, base_url: &str) -> String {
    let parsed = reqwest::Url::parse(base_url).ok();
    let host = parsed
        .as_ref()
        .and_then(reqwest::Url::host_str)
        .unwrap_or_default();
    let port = parsed
        .as_ref()
        .and_then(reqwest::Url::port_or_known_default);

    if host.eq_ignore_ascii_case("ollama") || port == Some(11434) {
        return "ollama".to_string();
    }

    if host.contains("openai.com") {
        return "openai".to_string();
    }

    if !host.is_empty() && host != "host.docker.internal" && host != "localhost" {
        return host.split('.').next().unwrap_or(host).to_string();
    }

    provider_type.to_string()
}

fn mask_redis_url_for_logs(redis_url: &str) -> String {
    if let Some((scheme, rest)) = redis_url.split_once("://") {
        if let Some((userinfo, host_part)) = rest.rsplit_once('@') {
            if userinfo.contains(':') {
                return format!("{scheme}://:****@{host_part}");
            }

            return format!("{scheme}://****@{host_part}");
        }
    }

    redis_url.to_string()
}

fn log_startup_summary(cfg: &Config) {
    tracing::info!(
        product = release::PRODUCT_NAME,
        version = release::PRODUCT_VERSION,
        release = release::RELEASE_TITLE,
        compatibility_model = release::COMPATIBILITY_MODEL,
        "=== AI Cost Firewall Startup ==="
    );

    tracing::info!("Configuration:");
    for line in cfg.startup_summary_lines() {
        tracing::info!("{}", line);
    }

    tracing::info!("Dependency checks:");
}

fn log_guard_pipeline_summary(cfg: &Config) {
    let mode = match (cfg.security_guard_enabled, cfg.privacy_guard_enabled) {
        (false, false) => "core_only",
        (false, true) => "privacy_only",
        (true, false) => "security_only",
        (true, true) => "security_and_privacy",
    };

    tracing::info!(
        mode = mode,
        security_guard_enabled = cfg.security_guard_enabled,
        security_guard_url = %cfg.security_guard_url,
        privacy_guard_enabled = cfg.privacy_guard_enabled,
        privacy_guard_url = %cfg.privacy_guard_url,
        privacy_guard_mode = ?cfg.privacy_guard_mode,
        privacy_guard_restore_enabled = cfg.privacy_guard_restore_enabled,
        guard_fail_open = cfg.guard_fail_open,
        "guard orchestration pipeline selected"
    );

    if !cfg.privacy_guard_enabled && cfg.privacy_guard_restore_enabled {
        tracing::warn!(
            "privacy_guard_restore_enabled=true has no effect because privacy_guard_enabled=false"
        );
    }

    if cfg.security_guard_enabled && cfg.privacy_guard_enabled {
        tracing::info!(
            "guard order: security request scan -> privacy anonymize -> cache/upstream -> security response scan -> privacy restore"
        );
    } else if cfg.security_guard_enabled {
        tracing::info!(
            "guard order: security request scan -> cache/upstream -> security response scan"
        );
    } else if cfg.privacy_guard_enabled {
        tracing::info!("guard order: privacy anonymize -> cache/upstream -> privacy restore");
    } else {
        tracing::info!("guard order: cache/upstream only");
    }
}

pub struct RuntimeBuild {
    pub chat_service: Arc<ChatService>,
    pub dependencies: DependencyState,
    pub evidence_delivery: Option<BufferedHttpEvidenceHandle>,
}

pub async fn build_runtime(cfg: &Config) -> Result<RuntimeBuild> {
    log_startup_summary(cfg);

    let mut redis_available = false;
    let mut qdrant_available = false;

    let exact_cache: Arc<dyn ExactCache> = if !cfg.exact_cache_enabled {
        tracing::info!("[SKIP] Exact cache disabled; Redis is not required");
        Arc::new(NoopExactCache)
    } else {
        let redis_client = redis::Client::open(cfg.redis_url.clone()).with_context(|| {
            format!(
                "failed to create Redis client from redis_url '{}'",
                mask_redis_url_for_logs(&cfg.redis_url)
            )
        })?;

        match ConnectionManager::new(redis_client).await {
            Ok(conn) => {
                redis_available = true;
                tracing::info!(
                    redis_url = %mask_redis_url_for_logs(&cfg.redis_url),
                    "[OK] Redis connected"
                );
                Arc::new(RedisExactCache::new(conn, cfg.exact_cache_ttl_seconds))
            }
            Err(e) if cfg.exact_cache_fail_open => {
                tracing::warn!(
                    redis_url = %mask_redis_url_for_logs(&cfg.redis_url),
                    error = %e,
                    "Redis exact cache unavailable; exact_cache_fail_open=true so startup continues without exact cache"
                );
                Arc::new(NoopExactCache)
            }
            Err(e) => {
                tracing::error!(
                    redis_url = %mask_redis_url_for_logs(&cfg.redis_url),
                    error = %e,
                    "failed to connect to Redis exact cache"
                );

                return Err(e).with_context(|| {
                    format!(
                        "failed to connect to Redis exact cache using redis_url '{}'",
                        mask_redis_url_for_logs(&cfg.redis_url)
                    )
                });
            }
        }
    };

    tracing::info!(
        upstream_provider = cfg.upstream_provider.as_str(),
        upstream_base_url = %cfg.upstream_base_url,
        "checking upstream provider configuration"
    );

    let upstream = build_llm_upstream(cfg).context("failed to initialize upstream provider")?;
    let upstream_available = true;

    tracing::info!(
        upstream_provider = cfg.upstream_provider.as_str(),
        upstream_base_url = %cfg.upstream_base_url,
        "[OK] Upstream provider initialized"
    );

    let semantic_cache: Arc<dyn SemanticCache> = if cfg.semantic_cache_enabled {
        tracing::info!(
            embedding_provider = cfg.embedding_provider.as_str(),
            embedding_base_url = %cfg.embedding_base_url,
            embedding_model = %cfg.embedding_model,
            "initializing embedding provider"
        );

        let embedder: Arc<dyn EmbeddingProvider> = match cfg.embedding_provider {
            ProviderKind::OpenAiCompatible => {
                let provider = OpenAiEmbeddingProvider::new(
                    cfg.embedding_base_url.clone(),
                    cfg.embedding_api_key.clone(),
                    cfg.embedding_model.clone(),
                    Duration::from_secs(cfg.embedding_timeout_seconds),
                )
                .context("failed to initialize OpenAI-compatible embedding provider")?;

                tracing::info!(
                    embedding_provider = cfg.embedding_provider.as_str(),
                    embedding_base_url = %cfg.embedding_base_url,
                    embedding_model = %cfg.embedding_model,
                    "[OK] Embedding provider initialized"
                );

                Arc::new(provider)
            }
        };

        tracing::info!(
            qdrant_url = %cfg.qdrant_url,
            qdrant_collection = %cfg.qdrant_collection,
            qdrant_vector_size = cfg.qdrant_vector_size,
            "checking Qdrant semantic cache"
        );

        match QdrantSemanticCache::new(
            cfg.qdrant_url.clone(),
            cfg.qdrant_api_key.clone(),
            cfg.qdrant_collection.clone(),
            cfg.qdrant_vector_size,
            cfg.semantic_similarity_threshold,
            cfg.semantic_cache_retention_seconds,
            embedder,
        )
        .await
        {
            Ok(cache) => {
                qdrant_available = true;
                tracing::info!(
                    qdrant_url = %cfg.qdrant_url,
                    qdrant_collection = %cfg.qdrant_collection,
                    qdrant_vector_size = cfg.qdrant_vector_size,
                    "[OK] Qdrant connected and collection validated"
                );

                Arc::new(cache)
            }
            Err(e) => {
                tracing::error!(
                    qdrant_url = %cfg.qdrant_url,
                    qdrant_collection = %cfg.qdrant_collection,
                    qdrant_vector_size = cfg.qdrant_vector_size,
                    error = ?e,
                    "failed to initialize Qdrant semantic cache"
                );

                if cfg.semantic_cache_fail_open {
                    tracing::warn!(
                        "semantic_cache_fail_open=true; startup continues without semantic cache"
                    );
                    Arc::new(NoopSemanticCache)
                } else {
                    return Err(e).with_context(|| {
                        format!(
                            "failed to initialize Qdrant semantic cache using qdrant_url '{}' and collection '{}'",
                            cfg.qdrant_url, cfg.qdrant_collection
                        )
                    });
                }
            }
        }
    } else {
        tracing::info!("[SKIP] Semantic cache disabled; Qdrant and embeddings are not required");
        Arc::new(NoopSemanticCache)
    };

    log_guard_pipeline_summary(cfg);

    let chat_service_settings = ChatServiceSettings {
        semantic_cache_enabled: cfg.semantic_cache_enabled,
        exact_cache_enabled: cfg.exact_cache_enabled,
        exact_cache_fail_open: cfg.exact_cache_fail_open,
        exact_cache_store_enabled: cfg.exact_cache_store_enabled,
        semantic_cache_store_enabled: cfg.semantic_cache_store_enabled,
        semantic_cache_fail_open: cfg.semantic_cache_fail_open,
        max_prompt_chars: Some(cfg.max_prompt_chars),
    };

    let guard_orchestrator = build_guard_orchestrator(cfg);
    tracing::info!("[OK] Guard orchestrator initialized");

    let (evidence_sink, evidence_delivery): (
        Arc<dyn EvidenceSink>,
        Option<BufferedHttpEvidenceHandle>,
    ) = if cfg.audit_enabled {
        let endpoint = format!("{}/v1/events/batch", cfg.audit_url.trim_end_matches('/'));
        let (sink, handle) = BufferedHttpEvidenceSink::spawn(BufferedHttpEvidenceSettings {
            endpoint: endpoint.clone(),
            api_key: cfg.audit_api_key.clone(),
            producer_instance_id: cfg.audit_producer_instance_id.clone(),
            queue_capacity: cfg.audit_queue_capacity,
            batch_size: cfg.audit_batch_size,
            flush_interval: Duration::from_millis(cfg.audit_flush_interval_ms),
            request_timeout: Duration::from_secs(cfg.audit_timeout_seconds),
            retry_max_attempts: cfg.audit_retry_max_attempts,
            retry_initial_backoff: Duration::from_millis(cfg.audit_retry_initial_backoff_ms),
        })?;
        tracing::info!(
            audit_endpoint = %endpoint,
            producer_instance_id = %cfg.audit_producer_instance_id,
            queue_capacity = cfg.audit_queue_capacity,
            batch_size = cfg.audit_batch_size,
            "[OK] VCAL Audit buffered evidence delivery initialized"
        );
        (sink, Some(handle))
    } else {
        tracing::info!("VCAL Audit disabled; evidence events will be written to tracing logs");
        (Arc::new(TracingEvidenceSink), None)
    };

    let chat_service = Arc::new(ChatService::new_with_guards_and_evidence(
        ChatServiceDeps {
            exact_cache,
            semantic_cache,
            upstream,
            guard_orchestrator,
            evidence_sink,
            upstream_metadata: UpstreamMetadata {
                provider_type: cfg.upstream_provider.as_str().to_string(),
                provider_name: infer_upstream_provider_name(
                    cfg.upstream_provider.as_str(),
                    &cfg.upstream_base_url,
                ),
            },
        },
        chat_service_settings,
        cfg.model_prices.clone(),
        cfg.embedding_price.clone(),
    ));

    tracing::info!("[OK] Runtime initialized");

    Ok(RuntimeBuild {
        chat_service,
        dependencies: DependencyState::new(redis_available, qdrant_available, upstream_available),
        evidence_delivery,
    })
}

pub async fn build_app(config: Config) -> Result<BuiltApp> {
    metrics::init();

    let runtime = build_runtime(&config).await?;

    let state = Arc::new(AppState {
        config: Arc::new(RwLock::new(config.clone())),
        chat_service: Arc::new(RwLock::new(runtime.chat_service)),
        dependencies: runtime.dependencies,
        evidence_delivery: Arc::new(Mutex::new(runtime.evidence_delivery)),
        shutdown: ShutdownState::new(),
    });

    let router = Router::new()
        .route("/healthz", get(api::health))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics_handler))
        .route("/version", get(version))
        .route("/v1/chat/completions", post(api::chat::chat_completions))
        .layer(DefaultBodyLimit::max(config.max_request_body_bytes))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            shutdown_gate_middleware,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    Ok(BuiltApp { router, state })
}

async fn version() -> impl IntoResponse {
    let body = json!({
        "product": release::PRODUCT_NAME,
        "version": release::PRODUCT_VERSION,
        "release_title": release::RELEASE_TITLE,
        "supported_api_style": release::SUPPORTED_API_STYLE,
        "compatibility_model": release::COMPATIBILITY_MODEL,
        "provider_specific_config_blocks": false,
        "native_provider_integrations": false,
        "scope_note": release::SCOPE_NOTE
    });

    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        format!("{}\n", body),
    )
}

async fn metrics_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let cfg = state.config.read().await;

    if cfg.metrics_auth_required {
        let Some(expected_token) = cfg.metrics_auth_token.as_deref() else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "metrics authentication is enabled but metrics_auth_token is not configured\n",
            )
                .into_response();
        };

        let authorized = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(|value| value == format!("Bearer {expected_token}"))
            .unwrap_or(false);

        if !authorized {
            return (StatusCode::UNAUTHORIZED, "unauthorized\n").into_response();
        }
    }

    drop(cfg);
    api::metrics().await.into_response()
}

async fn readyz(State(state): State<Arc<AppState>>) -> (StatusCode, &'static str) {
    if !state.shutdown.is_ready() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "not ready: shutting down\n",
        );
    }

    let cfg = state.config.read().await;
    let requires_redis = cfg.readiness_requires_redis;
    let requires_qdrant = cfg.readiness_requires_qdrant;
    let requires_upstream = cfg.readiness_requires_upstream;
    drop(cfg);

    if requires_redis && !state.dependencies.redis_available.load(Ordering::Relaxed) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "not ready: redis unavailable\n",
        );
    }

    if requires_qdrant && !state.dependencies.qdrant_available.load(Ordering::Relaxed) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "not ready: qdrant unavailable\n",
        );
    }

    if requires_upstream
        && !state
            .dependencies
            .upstream_available
            .load(Ordering::Relaxed)
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "not ready: upstream unavailable\n",
        );
    }

    (StatusCode::OK, "ready\n")
}

async fn shutdown_gate_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path();
    let is_probe = matches!(path, "/healthz" | "/readyz" | "/metrics" | "/version");

    if state.shutdown.is_shutting_down() && !is_probe {
        metrics::SHUTDOWN_REJECTIONS_TOTAL.inc();

        tracing::debug!(path = %path, "request rejected because shutdown is in progress");

        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": {
                    "code": 503,
                    "message": "server is shutting down",
                    "type": "service_unavailable"
                }
            })),
        )
            .into_response();
    }

    state
        .shutdown
        .inflight_requests
        .fetch_add(1, Ordering::SeqCst);
    metrics::INFLIGHT_REQUESTS.inc();

    let response = next.run(req).await;

    state
        .shutdown
        .inflight_requests
        .fetch_sub(1, Ordering::SeqCst);
    metrics::INFLIGHT_REQUESTS.dec();

    response
}
