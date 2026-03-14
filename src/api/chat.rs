use crate::{
    app::AppState,
    error::AppError,
    metrics,
    types::openai::{ChatCompletionRequest, ChatCompletionResponse},
};
use axum::{extract::State, Json};
use std::sync::Arc;

pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatCompletionRequest>,
) -> Result<Json<ChatCompletionResponse>, AppError> {
    metrics::INFLIGHT_REQUESTS.inc();
    metrics::REQUESTS_TOTAL
        .with_label_values(&["/v1/chat/completions"])
        .inc();

    let service = {
        let guard = state.chat_service.read().await;
        guard.clone()
    };

    let result = service.handle(req).await;

    metrics::INFLIGHT_REQUESTS.dec();
    result.map(Json)
}
