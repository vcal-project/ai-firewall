use crate::{
    app::AppState,
    error::AppError,
    metrics,
    services::chat_service::CacheControl,
    types::openai::{ChatCompletionRequest, ChatCompletionResponse},
};
use axum::{
    extract::{rejection::JsonRejection, State},
    http::HeaderMap,
    Json,
};
use std::sync::Arc;
use uuid::Uuid;

pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    payload: Result<Json<ChatCompletionRequest>, JsonRejection>,
) -> Result<Json<ChatCompletionResponse>, AppError> {
    metrics::REQUESTS_TOTAL
        .with_label_values(&["/v1/chat/completions"])
        .inc();

    let Json(req) = match payload {
        Ok(json) => json,
        Err(rejection) => {
            let err = map_json_rejection(rejection);
            metrics::ERRORS_TOTAL
                .with_label_values(&[err.metrics_class()])
                .inc();
            return Err(err);
        }
    };

    if let Err(err) = validate_chat_request(&state, &req).await {
        metrics::ERRORS_TOTAL
            .with_label_values(&[err.metrics_class()])
            .inc();
        return Err(err);
    }

    let cache_control = cache_control_from_headers(&state, &headers).await;
    let service = state.chat_service().await;

    let trace_id = headers
        .get("x-vcal-trace-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .unwrap_or_else(Uuid::new_v4);

    match service
        .handle_with_evidence(req, cache_control, trace_id)
        .await
    {
        Ok(response) => Ok(Json(response)),
        Err(err) => {
            metrics::ERRORS_TOTAL
                .with_label_values(&[err.metrics_class()])
                .inc();
            Err(err)
        }
    }
}

async fn cache_control_from_headers(state: &Arc<AppState>, headers: &HeaderMap) -> CacheControl {
    let cfg = state.config.read().await;
    let bypass = headers
        .get(cfg.cache_bypass_header.as_str())
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false);

    if bypass {
        metrics::CACHE_BYPASS_REQUESTS_TOTAL.inc();
    }

    CacheControl {
        bypass_lookup: bypass,
        bypass_store: bypass,
    }
}

async fn validate_chat_request(
    state: &Arc<AppState>,
    req: &ChatCompletionRequest,
) -> Result<(), AppError> {
    let model = req.normalized_model();

    if model.is_empty() {
        return Err(AppError::bad_request("model must not be empty"));
    }

    let allow_unknown = state.allow_unknown_models_pass_through().await;
    let is_allowed = state.is_model_allowed(model).await;

    if !allow_unknown && !is_allowed {
        return Err(AppError::bad_request(format!(
            "unsupported model: {}",
            model
        )));
    }

    Ok(())
}

fn map_json_rejection(rejection: JsonRejection) -> AppError {
    let msg = rejection.body_text();
    let lower = msg.to_ascii_lowercase();

    if lower.contains("body too large")
        || lower.contains("length limit exceeded")
        || lower.contains("request body too large")
    {
        return AppError::payload_too_large("request body exceeds max_request_body_bytes");
    }

    match rejection {
        JsonRejection::MissingJsonContentType(_) => {
            AppError::bad_request("content-type must be application/json")
        }
        JsonRejection::JsonSyntaxError(_) => {
            AppError::bad_request(format!("failed to parse request body as JSON: {msg}"))
        }
        JsonRejection::JsonDataError(_) => AppError::unprocessable(format!(
            "failed to deserialize JSON body into target type: {msg}"
        )),
        _ => AppError::bad_request(msg),
    }
}
