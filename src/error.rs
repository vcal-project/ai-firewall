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

    #[error("upstream timeout: {message}")]
    UpstreamTimeout { message: String },

    #[error("upstream error: {message}")]
    Upstream {
        status: Option<StatusCode>,
        message: String,
    },

    #[error("internal error: {message}")]
    Internal { message: String },
}

impl AppError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::Validation {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub fn payload_too_large(message: impl Into<String>) -> Self {
        Self::Validation {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: message.into(),
        }
    }

    pub fn unprocessable(message: impl Into<String>) -> Self {
        Self::Validation {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message: message.into(),
        }
    }

    pub fn upstream_timeout(message: impl Into<String>) -> Self {
        Self::UpstreamTimeout {
            message: message.into(),
        }
    }

    pub fn upstream(message: impl Into<String>) -> Self {
        Self::Upstream {
            status: None,
            message: message.into(),
        }
    }

    pub fn upstream_with_status(status: StatusCode, message: impl Into<String>) -> Self {
        Self::Upstream {
            status: Some(status),
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }

    pub fn semantic_provider(message: impl Into<String>) -> Self {
        Self::Internal {
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

    /// Stable classification for Prometheus labels and Grafana panels.
    pub fn metrics_class(&self) -> &'static str {
        match self {
            AppError::Validation { .. } => "validation",
            AppError::UpstreamTimeout { .. } => "upstream_timeout",
            AppError::Upstream { .. } => "upstream",
            AppError::Internal { .. } => "internal",
        }
    }

    fn error_type(&self) -> &'static str {
        match self {
            AppError::Validation { status, .. } => {
                if *status == StatusCode::PAYLOAD_TOO_LARGE {
                    "payload_too_large"
                } else {
                    "validation_error"
                }
            }
            AppError::UpstreamTimeout { .. } => "upstream_timeout",
            AppError::Upstream { .. } => "upstream_error",
            AppError::Internal { .. } => "internal_error",
        }
    }

    pub fn status_code(&self) -> StatusCode {
        match self {
            AppError::Validation { status, .. } => *status,
            AppError::UpstreamTimeout { .. } => StatusCode::GATEWAY_TIMEOUT,
            AppError::Upstream { status, .. } => status.unwrap_or(StatusCode::BAD_GATEWAY),
            AppError::Internal { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            AppError::Validation { message, .. } => message,
            AppError::UpstreamTimeout { message } => message,
            AppError::Upstream { message, .. } => message,
            AppError::Internal { message } => message,
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
