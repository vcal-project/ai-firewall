use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("upstream error: {0}")]
    Upstream(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    #[allow(dead_code)]
    pub fn upstream_json(status: StatusCode, body: String) -> Response {
        match serde_json::from_str::<Value>(&body) {
            Ok(v) => (status, Json(v)).into_response(),
            Err(_) => (
                status,
                Json(json!({
                    "error": {
                        "message": body,
                        "type": "upstream_error",
                        "code": status.as_u16()
                    }
                })),
            )
                .into_response(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Upstream(msg) => (StatusCode::BAD_GATEWAY, msg),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        (
            status,
            Json(json!({
                "error": {
                    "message": msg,
                    "type": "ai_firewall_error",
                    "code": status.as_u16()
                }
            })),
        )
            .into_response()
    }
}
