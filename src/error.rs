use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::upstream::llm::UpstreamErrorKind;

/// Stable dependency taxonomy used by metrics/evidence.
///
/// Some variants are reserved for dependencies that still use dedicated
/// `AppError` variants today. Keeping them here avoids changing metric/evidence
/// vocabulary when those paths are migrated to `DependencyFailure`.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DependencyKind {
    Redis,
    Qdrant,
    Upstream,
    SecurityGuard,
    PrivacyGuard,
    Audit,
}

impl DependencyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Redis => "redis",
            Self::Qdrant => "qdrant",
            Self::Upstream => "upstream",
            Self::SecurityGuard => "security_guard",
            Self::PrivacyGuard => "privacy_guard",
            Self::Audit => "audit",
        }
    }
}

/// Stable failure taxonomy used by metrics/evidence.
///
/// Only `Unavailable` is currently constructed through `DependencyFailure`;
/// the remaining classes are reserved for incremental migration of upstream
/// and guard failures without changing their public labels later.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureClass {
    Timeout,
    Connection,
    Dns,
    Tls,
    Authentication,
    RateLimit,
    Unavailable,
    Contract,
    MalformedResponse,
    Internal,
}

impl FailureClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Connection => "connection",
            Self::Dns => "dns",
            Self::Tls => "tls",
            Self::Authentication => "authentication",
            Self::RateLimit => "rate_limit",
            Self::Unavailable => "unavailable",
            Self::Contract => "contract",
            Self::MalformedResponse => "malformed_response",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("validation error: {message}")]
    Validation { status: StatusCode, message: String },

    #[error("upstream error: {message}")]
    Upstream {
        status: Option<StatusCode>,
        kind: UpstreamErrorKind,
        message: String,
        hint: Option<String>,
    },

    #[error("security guard blocked request: {message}")]
    SecurityRequestBlocked {
        message: String,
        rule_id: Option<String>,
    },

    #[error("security guard blocked response: {message}")]
    SecurityResponseBlocked {
        message: String,
        rule_id: Option<String>,
    },

    #[error("security guard unavailable during {stage}: {message}")]
    SecurityGuardUnavailable {
        stage: &'static str,
        message: String,
    },

    #[error("security guard timeout during {stage}: {message}")]
    SecurityGuardTimeout {
        stage: &'static str,
        message: String,
    },

    #[error("privacy guard anonymization failed: {message}")]
    PrivacyAnonymizationFailed { message: String },

    #[error("privacy guard restore failed: {message}")]
    PrivacyRestoreFailed { message: String },

    #[error("guard contract violation: {message}")]
    GuardContractViolation { message: String },

    #[error("dependency {dependency:?} failure ({class:?}): {message}")]
    DependencyFailure {
        dependency: DependencyKind,
        class: FailureClass,
        message: String,
    },

    #[error("backpressure limit reached for {scope}: {message}")]
    Backpressure {
        scope: &'static str,
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

    pub fn security_request_blocked(message: impl Into<String>, rule_id: Option<String>) -> Self {
        Self::SecurityRequestBlocked {
            message: message.into(),
            rule_id,
        }
    }

    pub fn security_response_blocked(message: impl Into<String>, rule_id: Option<String>) -> Self {
        Self::SecurityResponseBlocked {
            message: message.into(),
            rule_id,
        }
    }

    pub fn security_guard_unavailable(stage: &'static str, message: impl Into<String>) -> Self {
        Self::SecurityGuardUnavailable {
            stage,
            message: message.into(),
        }
    }

    pub fn security_guard_timeout(stage: &'static str, message: impl Into<String>) -> Self {
        Self::SecurityGuardTimeout {
            stage,
            message: message.into(),
        }
    }

    pub fn privacy_anonymization_failed(message: impl Into<String>) -> Self {
        Self::PrivacyAnonymizationFailed {
            message: message.into(),
        }
    }

    pub fn privacy_restore_failed(message: impl Into<String>) -> Self {
        Self::PrivacyRestoreFailed {
            message: message.into(),
        }
    }

    pub fn guard_contract_violation(message: impl Into<String>) -> Self {
        Self::GuardContractViolation {
            message: message.into(),
        }
    }

    pub fn upstream_kind(kind: UpstreamErrorKind, message: impl Into<String>) -> Self {
        Self::Upstream {
            status: None,
            kind,
            message: message.into(),
            hint: kind.default_hint().map(str::to_string),
        }
    }

    pub fn upstream_kind_with_status(
        status: StatusCode,
        kind: UpstreamErrorKind,
        message: impl Into<String>,
    ) -> Self {
        Self::Upstream {
            status: Some(status),
            kind,
            message: message.into(),
            hint: kind.default_hint().map(str::to_string),
        }
    }

    pub fn dependency_failure(
        dependency: DependencyKind,
        class: FailureClass,
        message: impl Into<String>,
    ) -> Self {
        Self::DependencyFailure {
            dependency,
            class,
            message: message.into(),
        }
    }

    pub fn dependency_labels(&self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::DependencyFailure {
                dependency, class, ..
            } => Some((dependency.as_str(), class.as_str())),
            Self::Upstream { kind, .. } => Some((
                "upstream",
                match kind {
                    UpstreamErrorKind::Timeout => "timeout",
                    UpstreamErrorKind::Tls => "tls",
                    UpstreamErrorKind::Dns => "dns",
                    UpstreamErrorKind::Connect => "connection",
                    UpstreamErrorKind::Authentication => "authentication",
                    UpstreamErrorKind::RateLimited => "rate_limit",
                    UpstreamErrorKind::NotFound | UpstreamErrorKind::HttpStatus => "unavailable",
                    UpstreamErrorKind::Other => "malformed_response",
                },
            )),
            Self::SecurityGuardTimeout { .. } => Some(("security_guard", "timeout")),
            Self::SecurityGuardUnavailable { .. } => Some(("security_guard", "unavailable")),
            Self::PrivacyAnonymizationFailed { .. } | Self::PrivacyRestoreFailed { .. } => {
                Some(("privacy_guard", "unavailable"))
            }
            Self::GuardContractViolation { .. } => Some(("guard", "contract")),
            _ => None,
        }
    }

    pub fn backpressure(scope: &'static str, message: impl Into<String>) -> Self {
        Self::Backpressure {
            scope,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
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

    /// Stable, payload-free reason code for VCAL Audit and Compliance evidence.
    pub fn evidence_reason_code(&self) -> &'static str {
        match self {
            AppError::Upstream { kind, .. } => match kind {
                UpstreamErrorKind::Dns => "UPSTREAM_DNS_ERROR",
                UpstreamErrorKind::Connect | UpstreamErrorKind::Tls => "UPSTREAM_CONNECT_ERROR",
                UpstreamErrorKind::Timeout => "UPSTREAM_TIMEOUT",
                UpstreamErrorKind::Authentication
                | UpstreamErrorKind::NotFound
                | UpstreamErrorKind::RateLimited
                | UpstreamErrorKind::HttpStatus => "UPSTREAM_HTTP_ERROR",
                UpstreamErrorKind::Other => "UPSTREAM_RESPONSE_DECODE_ERROR",
            },
            AppError::Backpressure { .. } => "BACKPRESSURE_LIMIT",
            AppError::DependencyFailure {
                dependency, class, ..
            } => match (dependency, class) {
                (DependencyKind::Redis, _) => "REDIS_DEPENDENCY_ERROR",
                (DependencyKind::Qdrant, _) => "QDRANT_DEPENDENCY_ERROR",
                (DependencyKind::SecurityGuard, _) => "SECURITY_GUARD_DEPENDENCY_ERROR",
                (DependencyKind::PrivacyGuard, _) => "PRIVACY_GUARD_DEPENDENCY_ERROR",
                (DependencyKind::Audit, _) => "AUDIT_DEPENDENCY_ERROR",
                (DependencyKind::Upstream, FailureClass::Timeout) => "UPSTREAM_TIMEOUT",
                (DependencyKind::Upstream, _) => "UPSTREAM_DEPENDENCY_ERROR",
            },
            _ => "REQUEST_PROCESSING_ERROR",
        }
    }

    /// Stable classification for Prometheus labels and Grafana panels.
    pub fn metrics_class(&self) -> &'static str {
        match self {
            AppError::Validation { .. } => "validation",
            AppError::Upstream { kind, .. } => kind.metrics_class(),
            AppError::SecurityRequestBlocked { .. } => "security_request_blocked",
            AppError::SecurityResponseBlocked { .. } => "security_response_blocked",
            AppError::SecurityGuardUnavailable { .. } => "security_guard_unavailable",
            AppError::SecurityGuardTimeout { .. } => "security_guard_timeout",
            AppError::PrivacyAnonymizationFailed { .. } => "privacy_anonymization_failed",
            AppError::PrivacyRestoreFailed { .. } => "privacy_restore_failed",
            AppError::GuardContractViolation { .. } => "guard_contract_violation",
            AppError::DependencyFailure { .. } => "dependency_failure",
            AppError::Backpressure { .. } => "backpressure",
            AppError::Internal { .. } => "internal",
        }
    }

    /// Returns true only for intentional Security Guard policy blocks.
    ///
    /// Transport failures, timeouts, and contract violations are operational
    /// errors and must continue to be recorded with `result="error"`.
    pub fn is_security_block(&self) -> bool {
        matches!(
            self,
            AppError::SecurityRequestBlocked { .. } | AppError::SecurityResponseBlocked { .. }
        )
    }

    /// Returns the request/response stage for an intentional Security Guard block.
    pub fn security_block_stage(&self) -> Option<&'static str> {
        match self {
            AppError::SecurityRequestBlocked { .. } => Some("request"),
            AppError::SecurityResponseBlocked { .. } => Some("response"),
            _ => None,
        }
    }

    /// Returns the rule ID associated with an intentional Security Guard block.
    ///
    /// The value is borrowed from the structured error so orchestration code can
    /// preserve it in `aif_security_blocks_total` instead of using `unknown`.
    pub fn security_block_rule_id(&self) -> Option<&str> {
        match self {
            AppError::SecurityRequestBlocked { rule_id, .. }
            | AppError::SecurityResponseBlocked { rule_id, .. } => rule_id.as_deref(),
            _ => None,
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
            AppError::Upstream { kind, .. } => kind.as_str(),
            AppError::SecurityRequestBlocked { .. } => "security_request_blocked",
            AppError::SecurityResponseBlocked { .. } => "security_response_blocked",
            AppError::SecurityGuardUnavailable { .. } => "security_guard_unavailable",
            AppError::SecurityGuardTimeout { .. } => "security_guard_timeout",
            AppError::PrivacyAnonymizationFailed { .. } => "privacy_anonymization_failed",
            AppError::PrivacyRestoreFailed { .. } => "privacy_restore_failed",
            AppError::GuardContractViolation { .. } => "guard_contract_violation",
            AppError::DependencyFailure { .. } => "dependency_failure",
            AppError::Backpressure { .. } => "backpressure",
            AppError::Internal { .. } => "internal_error",
        }
    }

    pub fn status_code(&self) -> StatusCode {
        match self {
            AppError::Validation { status, .. } => *status,
            AppError::Upstream { status, kind, .. } => match kind {
                UpstreamErrorKind::Timeout => StatusCode::GATEWAY_TIMEOUT,
                UpstreamErrorKind::Authentication => StatusCode::BAD_GATEWAY,
                UpstreamErrorKind::NotFound => StatusCode::BAD_GATEWAY,
                UpstreamErrorKind::RateLimited => StatusCode::BAD_GATEWAY,
                _ => status.unwrap_or(StatusCode::BAD_GATEWAY),
            },
            AppError::SecurityRequestBlocked { .. } => StatusCode::FORBIDDEN,
            AppError::SecurityResponseBlocked { .. } => StatusCode::FORBIDDEN,
            AppError::SecurityGuardUnavailable { .. } => StatusCode::BAD_GATEWAY,
            AppError::SecurityGuardTimeout { .. } => StatusCode::GATEWAY_TIMEOUT,
            AppError::PrivacyAnonymizationFailed { .. } => StatusCode::BAD_GATEWAY,
            AppError::PrivacyRestoreFailed { .. } => StatusCode::BAD_GATEWAY,
            AppError::GuardContractViolation { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::DependencyFailure { .. } | AppError::Backpressure { .. } => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            AppError::Internal { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            AppError::Validation { message, .. } => message,
            AppError::Upstream { message, .. } => message,
            AppError::SecurityRequestBlocked { message, .. } => message,
            AppError::SecurityResponseBlocked { message, .. } => message,
            AppError::SecurityGuardUnavailable { message, .. } => message,
            AppError::SecurityGuardTimeout { message, .. } => message,
            AppError::PrivacyAnonymizationFailed { message } => message,
            AppError::PrivacyRestoreFailed { message } => message,
            AppError::GuardContractViolation { message } => message,
            AppError::DependencyFailure { message, .. }
            | AppError::Backpressure { message, .. } => message,
            AppError::Internal { message } => message,
        }
    }

    pub fn hint(&self) -> Option<&str> {
        match self {
            AppError::Upstream { hint, .. } => hint.as_deref(),
            _ => None,
        }
    }

    fn guard_stage(&self) -> Option<&'static str> {
        match self {
            AppError::SecurityGuardUnavailable { stage, .. }
            | AppError::SecurityGuardTimeout { stage, .. } => Some(*stage),
            AppError::SecurityRequestBlocked { .. } => Some("request"),
            AppError::SecurityResponseBlocked { .. } => Some("response"),
            AppError::PrivacyAnonymizationFailed { .. } => Some("request"),
            AppError::PrivacyRestoreFailed { .. } => Some("response"),
            _ => None,
        }
    }

    fn guard_name(&self) -> Option<&'static str> {
        match self {
            AppError::SecurityRequestBlocked { .. }
            | AppError::SecurityResponseBlocked { .. }
            | AppError::SecurityGuardUnavailable { .. }
            | AppError::SecurityGuardTimeout { .. } => Some("security"),
            AppError::PrivacyAnonymizationFailed { .. } | AppError::PrivacyRestoreFailed { .. } => {
                Some("privacy")
            }
            _ => None,
        }
    }

    fn rule_id(&self) -> Option<&str> {
        match self {
            AppError::SecurityRequestBlocked { rule_id, .. }
            | AppError::SecurityResponseBlocked { rule_id, .. } => rule_id.as_deref(),
            _ => None,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let error_type = self.error_type();
        let message = self.message().to_string();
        let hint = self.hint().map(str::to_string);

        let mut error = json!({
            "code": status.as_u16(),
            "message": message,
            "type": error_type
        });

        if let Some(hint) = hint {
            error["hint"] = json!(hint);
        }

        if let Some(guard) = self.guard_name() {
            error["guard"] = json!(guard);
        }

        if let Some(stage) = self.guard_stage() {
            error["stage"] = json!(stage);
        }

        if let Some(rule_id) = self.rule_id() {
            error["rule_id"] = json!(rule_id);
        }

        (
            status,
            Json(json!({
                "error": error
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn response_json(error: AppError) -> Value {
        let response = error.into_response();
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("error response body should be readable");
        serde_json::from_slice(&body).expect("error response should be JSON")
    }

    #[tokio::test]
    async fn security_timeout_contract_is_stable() {
        let value = response_json(AppError::security_guard_timeout(
            "request",
            "security guard timed out",
        ))
        .await;

        assert_eq!(value["error"]["code"], 504);
        assert_eq!(value["error"]["type"], "security_guard_timeout");
        assert_eq!(value["error"]["guard"], "security");
        assert_eq!(value["error"]["stage"], "request");
    }

    #[tokio::test]
    async fn security_block_contract_preserves_rule_id() {
        let value = response_json(AppError::security_response_blocked(
            "blocked",
            Some("rule-123".to_string()),
        ))
        .await;

        assert_eq!(value["error"]["code"], 403);
        assert_eq!(value["error"]["type"], "security_response_blocked");
        assert_eq!(value["error"]["guard"], "security");
        assert_eq!(value["error"]["stage"], "response");
        assert_eq!(value["error"]["rule_id"], "rule-123");
    }

    #[tokio::test]
    async fn privacy_restore_failure_contract_is_stable() {
        let value = response_json(AppError::privacy_restore_failed("restore failed")).await;

        assert_eq!(value["error"]["code"], 502);
        assert_eq!(value["error"]["type"], "privacy_restore_failed");
        assert_eq!(value["error"]["guard"], "privacy");
        assert_eq!(value["error"]["stage"], "response");
    }
}
