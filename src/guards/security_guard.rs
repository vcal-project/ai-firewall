use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    error::AppError,
    types::openai::{ChatCompletionRequest, ChatCompletionResponse},
};

pub type GuardError = AppError;

const DEFAULT_SCAN_PATH: &str = "/v1/scan";

#[derive(Clone, Debug)]
pub struct SecurityGuardClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    scan_path: String,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityScanDirection {
    Request,
    Response,
}

#[derive(Debug, Serialize)]
struct SecurityScanRequest<'a> {
    direction: SecurityScanDirection,
    messages: Vec<SecurityScanMessage<'a>>,
}

#[derive(Debug, Serialize)]
struct SecurityScanMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct SecurityScanResponse {
    #[serde(default)]
    allowed: Option<bool>,
    #[serde(default)]
    blocked: Option<bool>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    status_code: Option<u16>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    rule_id: Option<String>,
    #[serde(default)]
    severity: Option<String>,
}

impl SecurityGuardClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        timeout_seconds: u64,
    ) -> Result<Self, GuardError> {
        Self::with_scan_path(base_url, api_key, timeout_seconds, DEFAULT_SCAN_PATH)
    }

    pub fn with_scan_path(
        base_url: impl Into<String>,
        api_key: Option<String>,
        timeout_seconds: u64,
        scan_path: impl Into<String>,
    ) -> Result<Self, GuardError> {
        let timeout = Duration::from_secs(timeout_seconds.max(1));
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| {
                AppError::internal(format!("failed to build Security Guard HTTP client: {e}"))
            })?;

        Ok(Self {
            http,
            base_url: normalize_base_url(base_url.into()),
            api_key,
            scan_path: normalize_scan_path(scan_path.into()),
        })
    }

    /// Scan the raw user request before Privacy Guard anonymization.
    pub async fn scan_request(&self, req: &ChatCompletionRequest) -> Result<(), GuardError> {
        let messages = req
            .messages
            .iter()
            .filter_map(|message| {
                extract_text_content(&message.content).map(|content| SecurityScanMessage {
                    role: message.role.as_str(),
                    content,
                })
            })
            .collect::<Vec<_>>();

        self.scan(SecurityScanDirection::Request, messages).await
    }

    /// Scan the current assistant response before it is returned.
    ///
    /// If Privacy Guard is enabled, this receives the anonymized response and
    /// should be called before Privacy Guard restore. If Privacy Guard is not
    /// enabled, this receives the original assistant response.
    pub async fn scan_response(&self, res: &ChatCompletionResponse) -> Result<(), GuardError> {
        let messages = res
            .choices
            .iter()
            .filter_map(|choice| {
                extract_text_content(&choice.message.content).map(|content| SecurityScanMessage {
                    role: choice.message.role.as_str(),
                    content,
                })
            })
            .collect::<Vec<_>>();

        self.scan(SecurityScanDirection::Response, messages).await
    }

    async fn scan(
        &self,
        direction: SecurityScanDirection,
        messages: Vec<SecurityScanMessage<'_>>,
    ) -> Result<(), GuardError> {
        if messages.is_empty() {
            tracing::debug!(
                ?direction,
                "Security Guard scan skipped; no string content found"
            );
            return Ok(());
        }

        let url = format!("{}{}", self.base_url, self.scan_path);
        let payload = SecurityScanRequest {
            direction,
            messages,
        };

        let mut request = self.http.post(&url).json(&payload);
        if let Some(api_key) = self.api_key.as_deref() {
            request = request.header("x-api-key", api_key);
        }

        let response = request.send().await.map_err(|e| {
            let stage_label = security_direction_label(direction);

            if e.is_timeout() {
                AppError::security_guard_timeout(
                    stage_label,
                    format!("Security Guard timeout during {stage_label} scan"),
                )
            } else {
                AppError::security_guard_unavailable(
                    stage_label,
                    format!("Security Guard request failed during {stage_label} scan: {e}"),
                )
            }
        })?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            let stage_label = security_direction_label(direction);

            AppError::security_guard_unavailable(
                stage_label,
                format!("failed to read Security Guard response during {stage_label} scan: {e}"),
            )
        })?;

        if !status.is_success() {
            return Err(security_guard_http_error(direction, status, &body));
        }

        let decision: SecurityScanResponse = serde_json::from_str(&body).map_err(|e| {
            let stage_label = security_direction_label(direction);

            AppError::guard_contract_violation(format!(
                "failed to decode Security Guard decision during {stage_label} scan: {e}; body={}",
                truncate_for_log(&body, 512)
            ))
        })?;

        if decision.is_blocked() {
            return Err(security_guard_blocked_error(direction, decision));
        }

        Ok(())
    }
}

impl SecurityScanResponse {
    fn is_blocked(&self) -> bool {
        if self.allowed == Some(false) {
            return true;
        }

        if self.blocked == Some(true) {
            return true;
        }

        if matches!(self.status_code, Some(401 | 403)) {
            return true;
        }

        if let Some(action) = &self.action {
            let action = action.to_ascii_lowercase();
            return matches!(
                action.as_str(),
                "block" | "blocked" | "deny" | "denied" | "reject" | "rejected"
            );
        }

        false
    }

    fn reason_or_default(&self, stage: SecurityScanDirection) -> String {
        self.reason
            .clone()
            .or_else(|| self.message.clone())
            .unwrap_or_else(|| format!("Security Guard blocked {:?} content", stage))
    }
}

fn security_guard_blocked_error(
    stage: SecurityScanDirection,
    decision: SecurityScanResponse,
) -> GuardError {
    let stage_label = security_direction_label(stage);

    let mut message = format!(
        "VCAL Security Guard blocked {stage_label}: {}",
        decision.reason_or_default(stage)
    );

    if let Some(rule_id) = decision.rule_id.as_deref() {
        message.push_str(&format!("; rule_id={rule_id}"));
    }

    if let Some(severity) = decision.severity.as_deref() {
        message.push_str(&format!("; severity={severity}"));
    }

    match stage {
        SecurityScanDirection::Request => {
            AppError::security_request_blocked(message, decision.rule_id)
        }
        SecurityScanDirection::Response => {
            AppError::security_response_blocked(message, decision.rule_id)
        }
    }
}

fn security_guard_http_error(
    stage: SecurityScanDirection,
    status: StatusCode,
    body: &str,
) -> GuardError {
    let body = truncate_for_log(body, 512);
    let stage_label = security_direction_label(stage);

    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            let message = format!(
                "VCAL Security Guard rejected {stage_label} scan with HTTP {status}: {body}"
            );

            match stage {
                SecurityScanDirection::Request => {
                    AppError::security_request_blocked(message, None)
                }
                SecurityScanDirection::Response => {
                    AppError::security_response_blocked(message, None)
                }
            }
        }

        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
            AppError::guard_contract_violation(format!(
                "VCAL Security Guard rejected AI Firewall {stage_label} scan payload with HTTP {status}: {body}"
            ))
        }

        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => {
            AppError::security_guard_timeout(
                stage_label,
                format!(
                    "VCAL Security Guard timed out during {stage_label} scan with HTTP {status}: {body}"
                ),
            )
        }

        _ if status.is_server_error() => {
            AppError::security_guard_unavailable(
                stage_label,
                format!(
                    "VCAL Security Guard returned HTTP {status} during {stage_label} scan: {body}"
                ),
            )
        }

        _ => AppError::security_guard_unavailable(
            stage_label,
            format!(
                "VCAL Security Guard returned unexpected HTTP {status} during {stage_label} scan: {body}"
            ),
        ),
    }
}

fn security_direction_label(stage: SecurityScanDirection) -> &'static str {
    match stage {
        SecurityScanDirection::Request => "request",
        SecurityScanDirection::Response => "response",
    }
}

fn normalize_base_url(mut base_url: String) -> String {
    while base_url.ends_with('/') {
        base_url.pop();
    }
    base_url
}

fn normalize_scan_path(mut path: String) -> String {
    if path.is_empty() {
        return DEFAULT_SCAN_PATH.to_string();
    }

    if !path.starts_with('/') {
        path.insert(0, '/');
    }

    path
}

fn extract_text_content(value: &Value) -> Option<&str> {
    match value {
        Value::String(s) if !s.is_empty() => Some(s.as_str()),
        _ => None,
    }
}

fn truncate_for_log(value: &str, max_chars: usize) -> String {
    let mut out = String::new();

    for (idx, ch) in value.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    fn blocked_decision() -> SecurityScanResponse {
        SecurityScanResponse {
            allowed: Some(false),
            blocked: None,
            action: None,
            status_code: Some(403),
            reason: Some("prompt injection detected".to_string()),
            message: None,
            rule_id: Some("prompt_injection_001".to_string()),
            severity: Some("high".to_string()),
        }
    }

    #[test]
    fn security_decision_detects_block_variants() {
        let by_allowed_false = SecurityScanResponse {
            allowed: Some(false),
            blocked: None,
            action: None,
            status_code: None,
            reason: None,
            message: None,
            rule_id: None,
            severity: None,
        };
        assert!(by_allowed_false.is_blocked());

        let by_blocked_true = SecurityScanResponse {
            allowed: None,
            blocked: Some(true),
            action: None,
            status_code: None,
            reason: None,
            message: None,
            rule_id: None,
            severity: None,
        };
        assert!(by_blocked_true.is_blocked());

        for action in ["block", "blocked", "deny", "denied", "reject", "rejected"] {
            let by_action = SecurityScanResponse {
                allowed: None,
                blocked: None,
                action: Some(action.to_string()),
                status_code: None,
                reason: None,
                message: None,
                rule_id: None,
                severity: None,
            };
            assert!(by_action.is_blocked(), "action={action} should block");
        }

        let allowed = SecurityScanResponse {
            allowed: Some(true),
            blocked: Some(false),
            action: Some("allow".to_string()),
            status_code: Some(200),
            reason: None,
            message: None,
            rule_id: None,
            severity: None,
        };
        assert!(!allowed.is_blocked());
    }

    #[test]
    fn request_block_maps_to_security_request_blocked_403() {
        let err = security_guard_blocked_error(SecurityScanDirection::Request, blocked_decision());

        assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
        assert_eq!(err.metrics_class(), "security_request_blocked");
        assert!(err.message().contains("prompt injection detected"));
        assert!(err.message().contains("prompt_injection_001"));
    }

    #[test]
    fn response_block_maps_to_security_response_blocked_403() {
        let err = security_guard_blocked_error(SecurityScanDirection::Response, blocked_decision());

        assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
        assert_eq!(err.metrics_class(), "security_response_blocked");
        assert!(err.message().contains("prompt injection detected"));
        assert!(err.message().contains("prompt_injection_001"));
    }

    #[test]
    fn security_guard_403_http_response_maps_to_request_block() {
        let err = security_guard_http_error(
            SecurityScanDirection::Request,
            StatusCode::FORBIDDEN,
            r#"{"error":"blocked by policy"}"#,
        );

        assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
        assert_eq!(err.metrics_class(), "security_request_blocked");
        assert!(err.message().contains("blocked by policy"));
    }

    #[test]
    fn security_guard_422_http_response_maps_to_guard_contract_violation() {
        let err = security_guard_http_error(
            SecurityScanDirection::Request,
            StatusCode::UNPROCESSABLE_ENTITY,
            r#"{"error":"invalid scan payload"}"#,
        );

        assert_eq!(err.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(err.metrics_class(), "guard_contract_violation");
        assert!(err.message().contains("invalid scan payload"));
    }

    #[test]
    fn security_guard_5xx_http_response_maps_to_unavailable_502() {
        let err = security_guard_http_error(
            SecurityScanDirection::Response,
            StatusCode::BAD_GATEWAY,
            "upstream security service failed",
        );

        assert_eq!(err.status_code(), StatusCode::BAD_GATEWAY);
        assert_eq!(err.metrics_class(), "security_guard_unavailable");
        assert!(err.message().contains("upstream security service failed"));
    }

    #[test]
    fn extract_text_content_only_returns_non_empty_strings() {
        assert_eq!(
            extract_text_content(&serde_json::json!("hello")),
            Some("hello")
        );
        assert_eq!(extract_text_content(&serde_json::json!("")), None);
        assert_eq!(extract_text_content(&serde_json::json!(["hello"])), None);
        assert_eq!(
            extract_text_content(&serde_json::json!({"type": "text", "text": "hello"})),
            None
        );
        assert_eq!(extract_text_content(&serde_json::Value::Null), None);
    }
}
