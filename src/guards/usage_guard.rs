use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{config::UsageGuardMode, error::AppError, types::openai::ChatCompletionRequest};

const DEFAULT_SCAN_PATH: &str = "/v1/scan";

#[derive(Clone, Debug)]
pub struct UsageGuardClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    tenant_id: Option<String>,
    policy_id: Option<String>,
    mode: UsageGuardMode,
    scan_path: String,
}

#[derive(Debug, Serialize)]
struct UsageScanRequest<'a> {
    request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tenant_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy_id: Option<&'a str>,
    mode: &'static str,
    direction: &'static str,
    messages: Vec<UsageScanMessage<'a>>,
}

#[derive(Debug, Serialize)]
struct UsageScanMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct UsageScanResponse {
    pub request_id: String,
    pub tenant_id: Option<String>,
    pub policy_id: Option<String>,
    pub policy_version: Option<String>,
    pub direction: String,
    pub classification: String,
    pub confidence: f32,
    pub category: Option<String>,
    pub decision: String,
    pub action: String,
    pub allowed: bool,
    pub blocked: bool,
    pub status_code: u16,
    pub reason: Option<String>,
    pub rule_id: Option<String>,
    #[serde(default)]
    pub findings: Vec<UsageFinding>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct UsageFinding {
    pub rule_id: String,
    pub classification: String,
    pub category: String,
    pub confidence: f32,
    pub source: String,
    pub reason: String,
}

impl UsageGuardClient {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        tenant_id: Option<String>,
        policy_id: Option<String>,
        mode: UsageGuardMode,
        timeout_seconds: u64,
    ) -> Result<Self, AppError> {
        let timeout = Duration::from_secs(timeout_seconds.max(1));
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| {
                AppError::internal(format!("failed to build Usage Guard HTTP client: {e}"))
            })?;

        Ok(Self {
            http,
            base_url: normalize_base_url(base_url.into()),
            api_key,
            tenant_id,
            policy_id,
            mode,
            scan_path: DEFAULT_SCAN_PATH.to_string(),
        })
    }

    pub async fn scan_request(
        &self,
        req: &ChatCompletionRequest,
        trace_id: Uuid,
    ) -> Result<Option<UsageScanResponse>, AppError> {
        let messages = req
            .messages
            .iter()
            .filter_map(|message| {
                extract_text_content(&message.content).map(|content| UsageScanMessage {
                    role: message.role.as_str(),
                    content,
                })
            })
            .collect::<Vec<_>>();

        if messages.is_empty() {
            tracing::debug!(
                guard = "usage",
                stage = "request",
                "Usage Guard scan skipped; no string content found"
            );
            return Ok(None);
        }

        let url = format!("{}{}", self.base_url, self.scan_path);
        let payload = UsageScanRequest {
            request_id: trace_id.to_string(),
            tenant_id: self.tenant_id.as_deref(),
            policy_id: self.policy_id.as_deref(),
            mode: self.mode.as_str(),
            direction: "request",
            messages,
        };

        let mut request = self.http.post(&url).json(&payload);
        if let Some(api_key) = self.api_key.as_deref() {
            if !api_key.trim().is_empty() {
                request = request.header("x-api-key", api_key);
            }
        }

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                AppError::usage_guard_timeout(
                    "request",
                    "VCAL Usage Guard timed out during request scan",
                )
            } else {
                AppError::usage_guard_unavailable(
                    "request",
                    format!("VCAL Usage Guard request scan failed: {e}"),
                )
            }
        })?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            AppError::usage_guard_unavailable(
                "request",
                format!("failed to read VCAL Usage Guard response: {e}"),
            )
        })?;

        if !status.is_success() {
            return Err(usage_guard_http_error(status, &body));
        }

        let decision: UsageScanResponse = serde_json::from_str(&body).map_err(|e| {
            AppError::guard_contract_violation(format!(
                "failed to decode VCAL Usage Guard decision: {e}; body={}",
                truncate_for_log(&body, 512)
            ))
        })?;

        Ok(Some(decision))
    }
}

impl UsageScanResponse {
    pub(crate) fn decision_label(&self) -> &str {
        self.decision.as_str()
    }

    pub(crate) fn should_stop(&self) -> bool {
        if self.blocked || !self.allowed || self.status_code == 403 {
            return true;
        }

        let decision = self.decision.to_ascii_lowercase();
        let action = self.action.to_ascii_lowercase();

        matches!(decision.as_str(), "block" | "blocked" | "escalate")
            || matches!(action.as_str(), "block" | "blocked" | "escalate")
    }

    pub(crate) fn reason_or_default(&self) -> String {
        self.reason.clone().unwrap_or_else(|| {
            if self.decision.eq_ignore_ascii_case("escalate")
                || self.action.eq_ignore_ascii_case("escalate")
            {
                "request requires escalation under organizational AI usage policy".to_string()
            } else {
                "request is not permitted by organizational AI usage policy".to_string()
            }
        })
    }
}

fn usage_guard_http_error(status: StatusCode, body: &str) -> AppError {
    let body = truncate_for_log(body, 512);

    match status {
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
            AppError::guard_contract_violation(format!(
                "VCAL Usage Guard rejected AI Firewall request scan payload with HTTP {status}: {body}"
            ))
        }
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => AppError::usage_guard_timeout(
            "request",
            format!("VCAL Usage Guard timed out during request scan with HTTP {status}: {body}"),
        ),
        _ => AppError::usage_guard_unavailable(
            "request",
            format!("VCAL Usage Guard returned HTTP {status} during request scan: {body}"),
        ),
    }
}

fn normalize_base_url(mut base_url: String) -> String {
    while base_url.ends_with('/') {
        base_url.pop();
    }
    base_url
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

    fn decision(decision: &str, action: &str, allowed: bool, blocked: bool) -> UsageScanResponse {
        UsageScanResponse {
            request_id: "test".to_string(),
            tenant_id: None,
            policy_id: Some("business-use-only".to_string()),
            policy_version: Some("1.0".to_string()),
            direction: "request".to_string(),
            classification: "disallowed_use".to_string(),
            confidence: 1.0,
            category: Some("personal_travel".to_string()),
            decision: decision.to_string(),
            action: action.to_string(),
            allowed,
            blocked,
            status_code: if blocked { 403 } else { 200 },
            reason: Some("outside approved business use".to_string()),
            rule_id: Some("VUG-PER-001".to_string()),
            findings: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn block_decision_stops_request() {
        assert!(decision("block", "block", false, true).should_stop());
    }

    #[test]
    fn escalate_decision_stops_request() {
        assert!(decision("escalate", "escalate", true, false).should_stop());
    }

    #[test]
    fn allow_decision_continues_request() {
        assert!(!decision("allow", "allow", true, false).should_stop());
    }

    #[test]
    fn usage_guard_422_maps_to_contract_violation() {
        let err = usage_guard_http_error(StatusCode::UNPROCESSABLE_ENTITY, "invalid scan payload");
        assert_eq!(err.metrics_class(), "guard_contract_violation");
    }

    #[test]
    fn usage_guard_403_http_maps_to_unavailable_not_policy_block() {
        let err = usage_guard_http_error(StatusCode::FORBIDDEN, "license or auth failure");
        assert_eq!(err.metrics_class(), "usage_guard_unavailable");
    }
}
