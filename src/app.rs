use crate::core::pricing::is_priced_model;

use crate::{
    api,
    cache::{exact::ExactCache, redis_exact::RedisExactCache},
    config::Config,
    embeddings::{openai::OpenAiEmbeddingProvider, provider::EmbeddingProvider},
    metrics,
    semantic::{
        noop::NoopSemanticCache, qdrant::QdrantSemanticCache, semantic_cache::SemanticCache,
    },
    services::chat_service::ChatService,
    upstream::{llm::LlmUpstream, openai::OpenAiUpstream},
};

use anyhow::Result;
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

pub async fn build_runtime(cfg: &Config) -> Result<Arc<ChatService>> {
    tracing::info!(
        semantic_cache_enabled = cfg.semantic_cache_enabled,
        request_timeout_seconds = cfg.request_timeout_seconds,
        cache_ttl_seconds = cfg.cache_ttl_seconds,
        "building application runtime"
    );

    let redis_client = redis::Client::open(cfg.redis_url.clone())?;
    let redis_conn = ConnectionManager::new(redis_client).await?;
    let exact_cache: Arc<dyn ExactCache> =
        Arc::new(RedisExactCache::new(redis_conn, cfg.cache_ttl_seconds));

    let upstream: Arc<dyn LlmUpstream> = Arc::new(OpenAiUpstream::new(
        cfg.upstream_base_url.clone(),
        cfg.upstream_api_key.clone(),
        Duration::from_secs(cfg.request_timeout_seconds),
    )?);

    let semantic_cache: Arc<dyn SemanticCache> = if cfg.semantic_cache_enabled {
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(OpenAiEmbeddingProvider::new(
            cfg.embedding_base_url.clone(),
            cfg.embedding_api_key.clone(),
            cfg.embedding_model.clone(),
            Duration::from_secs(cfg.request_timeout_seconds),
        )?);

        Arc::new(
            QdrantSemanticCache::new(
                cfg.qdrant_url.clone(),
                cfg.qdrant_api_key.clone(),
                cfg.qdrant_collection.clone(),
                cfg.qdrant_vector_size,
                cfg.semantic_similarity_threshold,
                cfg.cache_ttl_seconds,
                embedder,
            )
            .await?,
        )
    } else {
        Arc::new(NoopSemanticCache)
    };

    Ok(Arc::new(ChatService::new(
        exact_cache,
        semantic_cache,
        upstream,
        cfg.semantic_cache_enabled,
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

    tracing::info!("application router built successfully");

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
