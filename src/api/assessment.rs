use crate::{app::AppState, assessment::AssessmentContextV1, error::AppError};
use axum::{extract::State, Json};
use std::sync::Arc;

pub async fn get_assessment_context(
    State(state): State<Arc<AppState>>,
) -> Result<Json<AssessmentContextV1>, AppError> {
    let cfg = state.config.read().await;
    let context = AssessmentContextV1::from_config(&cfg).map_err(|error| {
        AppError::internal(format!(
            "failed to build assessment context from effective configuration: {error}"
        ))
    })?;

    Ok(Json(context))
}
