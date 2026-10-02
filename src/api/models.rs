use crate::{app::AppState, error::AppError, metrics};
use axum::{extract::State, Json};
use serde_json::Value;
use std::sync::Arc;

/// Proxy OpenAI-compatible model discovery to the configured chat/inference
/// upstream. This intentionally does not involve cache or guard execution.
pub async fn list_models(State(state): State<Arc<AppState>>) -> Result<Json<Value>, AppError> {
    metrics::REQUESTS_TOTAL
        .with_label_values(&["/v1/models"])
        .inc();

    let service = state.chat_service().await;
    match service.list_models().await {
        Ok(models) => Ok(Json(models)),
        Err(error) => {
            metrics::ERRORS_TOTAL
                .with_label_values(&[error.metrics_class()])
                .inc();
            if let Some((dependency, class)) = error.dependency_labels() {
                metrics::DEPENDENCY_FAILURES_TOTAL
                    .with_label_values(&[dependency, class])
                    .inc();
            }
            Err(error)
        }
    }
}
