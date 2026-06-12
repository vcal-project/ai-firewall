use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};
use uuid::Uuid;

use crate::{
    config::PrivacyGuardMode,
    error::AppError,
    guards::{privacy_mode_as_str, GuardContext, GuardOrchestrator, GuardedRequest},
    metrics,
    services::chat_service::CacheControl,
    types::openai::{ChatCompletionRequest, ChatCompletionResponse},
};

const GUARD_NAME: &str = "privacy";
const PHASE_SCAN: &str = "scan";
const PHASE_RESTORE: &str = "restore";

#[derive(Clone)]
pub struct PrivacyGuardOrchestrator {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    mode: PrivacyGuardMode,
    restore_enabled: bool,
    tenant_id: Option<String>,
    policy_id: Option<String>,
    fail_open: bool,
}

impl PrivacyGuardOrchestrator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base_url: String,
        api_key: Option<String>,
        mode: PrivacyGuardMode,
        restore_enabled: bool,
        tenant_id: Option<String>,
        policy_id: Option<String>,
        fail_open: bool,
        timeout: Duration,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "failed to build Privacy Guard HTTP client; using default client");
                reqwest::Client::new()
            });

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            mode,
            restore_enabled,
            tenant_id,
            policy_id,
            fail_open,
        }
    }

    fn request_builder(&self, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let builder = self.client.post(url);
        match self.api_key.as_deref() {
            Some(key) if !key.trim().is_empty() => builder.header("x-api-key", key),
            _ => builder,
        }
    }

    fn guard_unavailable(&self, phase: &'static str, error: String) -> Result<(), AppError> {
        metrics::GUARD_HOOK_ERRORS_TOTAL
            .with_label_values(&[GUARD_NAME, phase])
            .inc();

        if self.fail_open {
            tracing::warn!(
                guard = GUARD_NAME,
                phase = phase,
                error = %error,
                "Privacy Guard call failed; guard_fail_open=true so request continues unchanged"
            );
            Ok(())
        } else {
            Err(AppError::internal(format!(
                "Privacy Guard {phase} failed and guard_fail_open=false: {error}"
            )))
        }
    }

    fn restore_unavailable(&self, error: String) -> AppError {
        metrics::GUARD_HOOK_ERRORS_TOTAL
            .with_label_values(&[GUARD_NAME, PHASE_RESTORE])
            .inc();

        tracing::error!(
            guard = GUARD_NAME,
            phase = PHASE_RESTORE,
            error = %error,
            "Privacy Guard restore failed; failing closed to avoid returning placeholder-only output"
        );

        AppError::internal(format!(
            "Privacy Guard restore failed; response was not returned because restored output could not be produced: {error}"
        ))
    }
}

fn placeholder_signature_from_findings(findings: &[PrivacyFinding]) -> String {
    let mut counts = BTreeMap::from([
        ("EMAIL", 0usize),
        ("IP", 0usize),
        ("PHONE", 0usize),
        ("JWT", 0usize),
        ("API_KEY", 0usize),
        ("BEARER_TOKEN", 0usize),
        ("PRIVATE_KEY", 0usize),
        ("CREDIT_CARD_LIKE", 0usize),
        ("OTHER", 0usize),
    ]);

    for finding in findings {
        let key = finding.kind.signature_key();
        *counts.entry(key).or_insert(0) += finding.count;
    }

    [
        "EMAIL",
        "IP",
        "PHONE",
        "JWT",
        "API_KEY",
        "BEARER_TOKEN",
        "PRIVATE_KEY",
        "CREDIT_CARD_LIKE",
        "OTHER",
    ]
    .iter()
    .map(|key| format!("{}:{}", key, counts.get(*key).copied().unwrap_or(0)))
    .collect::<Vec<_>>()
    .join("|")
}

#[async_trait]
impl GuardOrchestrator for PrivacyGuardOrchestrator {
    fn reject_streaming_requests(&self) -> bool {
        true
    }

    async fn before_cache(
        &self,
        mut request: ChatCompletionRequest,
    ) -> Result<GuardedRequest, AppError> {
        metrics::GUARD_HOOK_CALLS_TOTAL
            .with_label_values(&[GUARD_NAME, PHASE_SCAN, "attempt"])
            .inc();

        let (messages, indexes) = collect_request_string_messages(&request);
        if messages.is_empty() {
            metrics::GUARD_HOOK_CALLS_TOTAL
                .with_label_values(&[GUARD_NAME, PHASE_SCAN, "skipped_no_string_content"])
                .inc();
            return Ok(GuardedRequest {
                request,
                context: GuardContext::default(),
                cache_control: CacheControl::default(),
            });
        }

        let scan_request = PrivacyScanRequest {
            request_id: Some(Uuid::new_v4().to_string()),
            tenant_id: self.tenant_id.clone(),
            conversation_id: None,
            policy_id: self.policy_id.clone(),
            mode: privacy_mode_as_str(self.mode).to_string(),
            messages: messages.clone(),
        };

        let response = match self
            .request_builder("/v1/scan")
            .json(&scan_request)
            .send()
            .await
        {
            Ok(response) => response,
            Err(e) => {
                self.guard_unavailable(PHASE_SCAN, e.to_string())?;
                return Ok(GuardedRequest {
                    request,
                    context: GuardContext::default(),
                    cache_control: CacheControl::default(),
                });
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            self.guard_unavailable(PHASE_SCAN, format!("HTTP {status}: {body}"))?;
            return Ok(GuardedRequest {
                request,
                context: GuardContext::default(),
                cache_control: CacheControl::default(),
            });
        }

        let scan = match response.json::<PrivacyScanResponse>().await {
            Ok(scan) => scan,
            Err(e) => {
                self.guard_unavailable(PHASE_SCAN, format!("invalid JSON response: {e}"))?;
                return Ok(GuardedRequest {
                    request,
                    context: GuardContext::default(),
                    cache_control: CacheControl::default(),
                });
            }
        };

        for finding in &scan.findings {
            metrics::GUARD_FINDINGS_TOTAL
                .with_label_values(&[GUARD_NAME, finding.kind.as_str(), finding.severity.as_str()])
                .inc_by(finding.count as u64);
        }

        if scan.decision == "block" {
            metrics::GUARD_REJECTIONS_TOTAL
                .with_label_values(&[GUARD_NAME])
                .inc();
            metrics::GUARD_HOOK_CALLS_TOTAL
                .with_label_values(&[GUARD_NAME, PHASE_SCAN, "block"])
                .inc();
            return Err(AppError::unprocessable(
                "request blocked by VCAL Privacy Guard",
            ));
        }

        if scan.modified {
            if let Err(error) =
                validate_privacy_message_contract(PHASE_SCAN, &messages, &scan.messages)
            {
                self.guard_unavailable(PHASE_SCAN, error)?;
                return Ok(GuardedRequest {
                    request,
                    context: GuardContext::default(),
                    cache_control: CacheControl::default(),
                });
            }

            apply_request_string_messages(&mut request, &indexes, &scan.messages);
            metrics::GUARD_TRANSFORMATIONS_TOTAL
                .with_label_values(&[GUARD_NAME, PHASE_SCAN, scan.action.as_str()])
                .inc();
        }

        let privacy_placeholder_signature = if scan.modified || scan.mapping_id.is_some() {
            Some(placeholder_signature_from_findings(&scan.findings))
        } else {
            None
        };

        if let Some(signature) = privacy_placeholder_signature.as_deref() {
            tracing::debug!(
                guard = GUARD_NAME,
                privacy_placeholder_signature = %signature,
                "Privacy Guard placeholder signature created for semantic cache isolation"
            );
        }

        if let Some(mapping_id) = &scan.mapping_id {
            metrics::GUARD_MAPPINGS_CREATED_TOTAL
                .with_label_values(&[GUARD_NAME])
                .inc();
            tracing::debug!(
                guard = GUARD_NAME,
                mapping_id = %mapping_id,
                "Privacy Guard returned placeholder mapping for response restoration"
            );
        }

        for warning in &scan.warnings {
            tracing::warn!(guard = GUARD_NAME, warning = %warning, "Privacy Guard warning");
        }

        metrics::GUARD_HOOK_CALLS_TOTAL
            .with_label_values(&[GUARD_NAME, PHASE_SCAN, "allow"])
            .inc();

        Ok(GuardedRequest {
            request,
            context: GuardContext {
                privacy_mapping_id: scan.mapping_id,
                privacy_tenant_id: scan.tenant_id.or_else(|| self.tenant_id.clone()),
                privacy_placeholder_signature,
            },
            cache_control: CacheControl::default(),
        })
    }

    async fn restore_response(
        &self,
        context: &GuardContext,
        mut response: ChatCompletionResponse,
    ) -> Result<ChatCompletionResponse, AppError> {
        let Some(mapping_id) = context.privacy_mapping_id.as_deref() else {
            return Ok(response);
        };

        if !self.restore_enabled {
            metrics::GUARD_HOOK_CALLS_TOTAL
                .with_label_values(&[GUARD_NAME, PHASE_RESTORE, "skipped_disabled"])
                .inc();
            return Ok(response);
        }

        let (messages, indexes) = collect_response_string_messages(&response);
        if messages.is_empty() {
            metrics::GUARD_HOOK_CALLS_TOTAL
                .with_label_values(&[GUARD_NAME, PHASE_RESTORE, "skipped_no_string_content"])
                .inc();
            return Ok(response);
        }

        metrics::GUARD_HOOK_CALLS_TOTAL
            .with_label_values(&[GUARD_NAME, PHASE_RESTORE, "attempt"])
            .inc();

        let restore_request = PrivacyRestoreRequest {
            request_id: Some(Uuid::new_v4().to_string()),
            tenant_id: context.privacy_tenant_id.clone(),
            mapping_id: mapping_id.to_string(),
            messages: messages.clone(),
        };

        let http_response = match self
            .request_builder("/v1/restore")
            .json(&restore_request)
            .send()
            .await
        {
            Ok(response) => response,
            Err(e) => {
                return Err(self.restore_unavailable(e.to_string()));
            }
        };

        if !http_response.status().is_success() {
            let status = http_response.status();
            let body = http_response.text().await.unwrap_or_default();
            return Err(self.restore_unavailable(format!("HTTP {status}: {body}")));
        }

        let restored = match http_response.json::<PrivacyRestoreResponse>().await {
            Ok(restored) => restored,
            Err(e) => {
                return Err(self.restore_unavailable(format!("invalid JSON response: {e}")));
            }
        };

        if restored.restored {
            if let Err(error) =
                validate_privacy_message_contract(PHASE_RESTORE, &messages, &restored.messages)
            {
                return Err(self.restore_unavailable(error));
            }

            apply_response_string_messages(&mut response, &indexes, &restored.messages);
            metrics::GUARD_TRANSFORMATIONS_TOTAL
                .with_label_values(&[GUARD_NAME, PHASE_RESTORE, "restore"])
                .inc();
        }

        for warning in &restored.warnings {
            tracing::warn!(guard = GUARD_NAME, warning = %warning, "Privacy Guard restore warning");
        }

        metrics::GUARD_HOOK_CALLS_TOTAL
            .with_label_values(&[GUARD_NAME, PHASE_RESTORE, "ok"])
            .inc();

        Ok(response)
    }
}

fn validate_privacy_message_contract(
    phase: &'static str,
    expected: &[PrivacyChatMessage],
    returned: &[PrivacyChatMessage],
) -> Result<(), String> {
    if expected.len() != returned.len() {
        return Err(format!(
            "Privacy Guard {phase} response contract violation: expected {} string messages, got {}",
            expected.len(),
            returned.len()
        ));
    }

    for (position, (expected_message, returned_message)) in
        expected.iter().zip(returned.iter()).enumerate()
    {
        if expected_message.role != returned_message.role {
            return Err(format!(
                "Privacy Guard {phase} response contract violation: role mismatch at string message position {position}: expected role '{}', got '{}'",
                expected_message.role,
                returned_message.role
            ));
        }
    }

    Ok(())
}

fn collect_request_string_messages(
    request: &ChatCompletionRequest,
) -> (Vec<PrivacyChatMessage>, Vec<usize>) {
    let mut messages = Vec::new();
    let mut indexes = Vec::new();

    for (idx, message) in request.messages.iter().enumerate() {
        match message.content.as_str() {
            Some(content) => {
                messages.push(PrivacyChatMessage {
                    role: message.role.clone(),
                    content: content.to_string(),
                });
                indexes.push(idx);
            }
            None => {
                metrics::GUARD_NON_STRING_CONTENT_TOTAL
                    .with_label_values(&[GUARD_NAME, PHASE_SCAN])
                    .inc();
                tracing::warn!(
                    guard = GUARD_NAME,
                    phase = PHASE_SCAN,
                    role = %message.role,
                    "leaving non-string chat message content unchanged; Privacy Guard v1 accepts string content only"
                );
            }
        }
    }

    (messages, indexes)
}

fn apply_request_string_messages(
    request: &mut ChatCompletionRequest,
    indexes: &[usize],
    messages: &[PrivacyChatMessage],
) {
    for (idx, message) in indexes.iter().copied().zip(messages.iter()) {
        if let Some(target) = request.messages.get_mut(idx) {
            target.content = Value::String(message.content.clone());
        }
    }
}

fn collect_response_string_messages(
    response: &ChatCompletionResponse,
) -> (Vec<PrivacyChatMessage>, Vec<usize>) {
    let mut messages = Vec::new();
    let mut indexes = Vec::new();

    for (idx, choice) in response.choices.iter().enumerate() {
        match choice.message.content.as_str() {
            Some(content) => {
                messages.push(PrivacyChatMessage {
                    role: choice.message.role.clone(),
                    content: content.to_string(),
                });
                indexes.push(idx);
            }
            None => {
                metrics::GUARD_NON_STRING_CONTENT_TOTAL
                    .with_label_values(&[GUARD_NAME, PHASE_RESTORE])
                    .inc();
                tracing::warn!(
                    guard = GUARD_NAME,
                    phase = PHASE_RESTORE,
                    role = %choice.message.role,
                    "leaving non-string assistant message content unchanged; Privacy Guard v1 restore accepts string content only"
                );
            }
        }
    }

    (messages, indexes)
}

fn apply_response_string_messages(
    response: &mut ChatCompletionResponse,
    indexes: &[usize],
    messages: &[PrivacyChatMessage],
) {
    for (idx, message) in indexes.iter().copied().zip(messages.iter()) {
        if let Some(choice) = response.choices.get_mut(idx) {
            choice.message.content = Value::String(message.content.clone());
        }
    }
}

#[derive(Debug, Serialize)]
struct PrivacyScanRequest {
    request_id: Option<String>,
    tenant_id: Option<String>,
    conversation_id: Option<String>,
    policy_id: Option<String>,
    mode: String,
    messages: Vec<PrivacyChatMessage>,
}

#[derive(Debug, Serialize)]
struct PrivacyRestoreRequest {
    request_id: Option<String>,
    tenant_id: Option<String>,
    mapping_id: String,
    messages: Vec<PrivacyChatMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PrivacyChatMessage {
    role: String,
    content: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct PrivacyScanResponse {
    request_id: String,
    tenant_id: Option<String>,
    policy_id: Option<String>,
    decision: String,
    action: String,
    modified: bool,
    mapping_id: Option<String>,
    messages: Vec<PrivacyChatMessage>,
    findings: Vec<PrivacyFinding>,
    warnings: Vec<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct PrivacyRestoreResponse {
    request_id: String,
    tenant_id: Option<String>,
    restored: bool,
    messages: Vec<PrivacyChatMessage>,
    warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PrivacyFinding {
    kind: PrivacyFindingKind,
    count: usize,
    severity: PrivacySeverity,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PrivacyFindingKind {
    Email,
    Ipv4,
    Phone,
    Jwt,
    ApiKey,
    BearerToken,
    PrivateKey,
    CreditCardLike,
    #[serde(other)]
    Other,
}

impl PrivacyFindingKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Ipv4 => "ipv4",
            Self::Phone => "phone",
            Self::Jwt => "jwt",
            Self::ApiKey => "api_key",
            Self::BearerToken => "bearer_token",
            Self::PrivateKey => "private_key",
            Self::CreditCardLike => "credit_card_like",
            Self::Other => "other",
        }
    }

    fn signature_key(&self) -> &'static str {
        match self {
            Self::Email => "EMAIL",
            Self::Ipv4 => "IP",
            Self::Phone => "PHONE",
            Self::Jwt => "JWT",
            Self::ApiKey => "API_KEY",
            Self::BearerToken => "BEARER_TOKEN",
            Self::PrivateKey => "PRIVATE_KEY",
            Self::CreditCardLike => "CREDIT_CARD_LIKE",
            Self::Other => "OTHER",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PrivacySeverity {
    Low,
    Medium,
    High,
    Critical,
    #[serde(other)]
    Other,
}

impl PrivacySeverity {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
            Self::Other => "other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str) -> PrivacyChatMessage {
        PrivacyChatMessage {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn privacy_message_contract_accepts_matching_len_and_roles() {
        let expected = vec![msg("system", "a"), msg("user", "b")];
        let returned = vec![msg("system", "x"), msg("user", "y")];

        assert!(validate_privacy_message_contract(PHASE_SCAN, &expected, &returned).is_ok());
    }

    #[test]
    fn privacy_message_contract_rejects_len_mismatch() {
        let expected = vec![msg("user", "a"), msg("assistant", "b")];
        let returned = vec![msg("user", "x")];

        let error = validate_privacy_message_contract(PHASE_SCAN, &expected, &returned)
            .expect_err("length mismatch must be rejected");

        assert!(error.contains("expected 2 string messages, got 1"));
    }

    #[test]
    fn privacy_message_contract_rejects_role_mismatch() {
        let expected = vec![msg("user", "a")];
        let returned = vec![msg("assistant", "x")];

        let error = validate_privacy_message_contract(PHASE_RESTORE, &expected, &returned)
            .expect_err("role mismatch must be rejected");

        assert!(error.contains("role mismatch at string message position 0"));
        assert!(error.contains("expected role 'user', got 'assistant'"));
    }

    #[test]
    fn restore_unavailable_fails_closed_even_when_guard_fail_open_is_true() {
        let guard = PrivacyGuardOrchestrator::new(
            "http://127.0.0.1:8090".to_string(),
            None,
            PrivacyGuardMode::Anonymize,
            true,
            None,
            None,
            true,
            Duration::from_secs(1),
        );

        let error = guard.restore_unavailable("simulated restore outage".to_string());

        assert!(error
            .message()
            .contains("Privacy Guard restore failed; response was not returned"));
        assert!(error.message().contains("simulated restore outage"));
    }
}
