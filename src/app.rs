use crate::core::pricing::is_priced_model;

use crate::{
    api,
    cache::{exact::ExactCache, redis_exact::RedisExactCache},
    config::{Config, ProviderKind},
    embeddings::{openai::OpenAiEmbeddingProvider, provider::EmbeddingProvider},
    metrics,
    semantic::{
        noop::NoopSemanticCache, qdrant::QdrantSemanticCache, semantic_cache::SemanticCache,
    },
    services::chat_service::ChatService,
    upstream::build_llm_upstream,
};

use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{Request, StatusCode},
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
use tokio::sync::RwLock;
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

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<Config>>,
    pub chat_service: Arc<RwLock<Arc<ChatService>>>,
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
    tracing::info!("=== AI Cost Firewall Startup ===");

    tracing::info!("Configuration:");
    for line in cfg.startup_summary_lines() {
        tracing::info!("{}", line);
    }

    tracing::info!("Dependency checks:");
}

pub async fn build_runtime(cfg: &Config) -> Result<Arc<ChatService>> {
    log_startup_summary(cfg);

    let redis_client = redis::Client::open(cfg.redis_url.clone()).with_context(|| {
        format!(
            "failed to create Redis client from redis_url '{}'",
            mask_redis_url_for_logs(&cfg.redis_url)
        )
    })?;

    let redis_conn = match ConnectionManager::new(redis_client).await {
        Ok(conn) => {
            tracing::info!(
                redis_url = %mask_redis_url_for_logs(&cfg.redis_url),
                "[OK] Redis connected"
            );
            conn
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
    };

    let exact_cache: Arc<dyn ExactCache> = Arc::new(RedisExactCache::new(
        redis_conn,
        cfg.exact_cache_ttl_seconds,
    ));

    tracing::info!(
        upstream_provider = cfg.upstream_provider.as_str(),
        upstream_base_url = %cfg.upstream_base_url,
        "checking upstream provider configuration"
    );

    let upstream = build_llm_upstream(cfg).context("failed to initialize upstream provider")?;

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
                    Duration::from_secs(cfg.request_timeout_seconds),
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

                return Err(e).with_context(|| {
                    format!(
                        "failed to initialize Qdrant semantic cache using qdrant_url '{}' and collection '{}'",
                        cfg.qdrant_url, cfg.qdrant_collection
                    )
                });
            }
        }
    } else {
        tracing::info!("[SKIP] Semantic cache disabled; Qdrant and embeddings are not required");
        Arc::new(NoopSemanticCache)
    };

    tracing::info!("[OK] Runtime initialized");

    Ok(Arc::new(ChatService::new(
        exact_cache,
        semantic_cache,
        upstream,
        cfg.semantic_cache_enabled,
        cfg.semantic_cache_fail_open,
        cfg.model_prices.clone(),
        cfg.embedding_price.clone(),
    )))
}

pub async fn build_app(config: Config) -> Result<BuiltApp> {
    metrics::init();

    let chat_service = build_runtime(&config).await?;

    let state = Arc::new(AppState {
        config: Arc::new(RwLock::new(config.clone())),
        chat_service: Arc::new(RwLock::new(chat_service)),
        shutdown: ShutdownState::new(),
    });

    let router = Router::new()
        .route("/healthz", get(api::health))
        .route("/readyz", get(readyz))
        .route("/metrics", get(api::metrics))
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

async fn readyz(State(state): State<Arc<AppState>>) -> StatusCode {
    if state.shutdown.is_ready() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

async fn shutdown_gate_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path();
    let is_probe = matches!(path, "/healthz" | "/readyz" | "/metrics");

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
