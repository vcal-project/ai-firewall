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

    let result = async {
        validate_chat_request(&state, &req).await?;

        let service = state.chat_service().await;
        let response = service.handle(req).await?;

        Ok::<Json<ChatCompletionResponse>, AppError>(Json(response))
    }
    .await;

    metrics::INFLIGHT_REQUESTS.dec();
    result
}

async fn validate_chat_request(
    state: &Arc<AppState>,
    req: &ChatCompletionRequest,
) -> Result<(), AppError> {
    let model = req.normalized_model();

    if model.is_empty() {
        return Err(AppError::bad_request("Model must not be empty"));
    }

    let allow_unknown = state.allow_unknown_models_pass_through().await;
    let is_allowed = state.is_model_allowed(model).await;

    if !allow_unknown && !is_allowed {
        return Err(AppError::bad_request(format!(
            "Unsupported model: {}",
            model
        )));
    }

    Ok(())
}
