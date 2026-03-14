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
    routing::{get, post},
    Router,
};
use redis::aio::ConnectionManager;
use std::{sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<Config>>,
    pub chat_service: Arc<RwLock<Arc<ChatService>>>,
}

pub struct BuiltApp {
    pub router: Router,
    pub state: Arc<AppState>,
}

pub async fn build_runtime(cfg: &Config) -> Result<Arc<ChatService>> {
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
    )))
}

pub async fn build_app(config: Config) -> Result<BuiltApp> {
    metrics::init();

    let chat_service = build_runtime(&config).await?;

    let state = Arc::new(AppState {
        config: Arc::new(RwLock::new(config)),
        chat_service: Arc::new(RwLock::new(chat_service)),
    });

    let router = Router::new()
        .route("/healthz", get(api::health))
        .route("/metrics", get(api::metrics))
        .route("/v1/chat/completions", post(api::chat::chat_completions))
        .with_state(state.clone())
        .layer(TraceLayer::new_for_http());

    Ok(BuiltApp { router, state })
}
