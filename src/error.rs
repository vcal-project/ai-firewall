use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("validation error: {message}")]
    Validation { status: StatusCode, message: String },

    #[error("upstream error: {0}")]
    Upstream(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::Validation {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    #[allow(dead_code)]
    pub fn upstream_json(status: StatusCode, body: String) -> Response {
        match serde_json::from_str::<Value>(&body) {
            Ok(v) => (status, Json(v)).into_response(),
            Err(_) => (
                status,
                Json(json!({
                    "error": {
                        "code": status.as_u16(),
                        "message": body,
                        "type": "upstream_error"
                    }
                })),
            )
                .into_response(),
        }
    }

    fn error_type(&self) -> &'static str {
        match self {
            AppError::Validation { .. } => "validation_error",
            AppError::Upstream(_) => "upstream_error",
            AppError::Internal(_) => "internal_error",
        }
    }

    fn status_code(&self) -> StatusCode {
        match self {
            AppError::Validation { status, .. } => *status,
            AppError::Upstream(_) => StatusCode::BAD_GATEWAY,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn message(&self) -> &str {
        match self {
            AppError::Validation { message, .. } => message,
            AppError::Upstream(message) => message,
            AppError::Internal(message) => message,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let error_type = self.error_type();
        let message = self.message().to_string();

        (
            status,
            Json(json!({
                "error": {
                    "code": status.as_u16(),
                    "message": message,
                    "type": error_type
                }
            })),
        )
            .into_response()
    }
}
