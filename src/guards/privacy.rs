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
            Err(AppError::privacy_anonymization_failed(format!(
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

        AppError::privacy_restore_failed(format!(
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
    async fn before_cache(
        &self,
        mut request: ChatCompletionRequest,
        _trace_id: Uuid,
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
                context: GuardContext {
                    privacy_mode: Some(privacy_mode_as_str(self.mode).to_string()),
                    privacy_scan_skipped: true,
                    ..GuardContext::default()
                },
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
                let reason = e.to_string();
                self.guard_unavailable(PHASE_SCAN, reason.clone())?;
                return Ok(GuardedRequest {
                    request,
                    context: GuardContext {
                        privacy_mode: Some(privacy_mode_as_str(self.mode).to_string()),
                        privacy_failure_reason: Some(reason),
                        ..GuardContext::default()
                    },
                    cache_control: CacheControl::default(),
                });
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let reason = format!("HTTP {status}: {body}");
            self.guard_unavailable(PHASE_SCAN, reason.clone())?;
            return Ok(GuardedRequest {
                request,
                context: GuardContext {
                    privacy_mode: Some(privacy_mode_as_str(self.mode).to_string()),
                    privacy_failure_reason: Some(reason),
                    ..GuardContext::default()
                },
                cache_control: CacheControl::default(),
            });
        }

        let scan = match response.json::<PrivacyScanResponse>().await {
            Ok(scan) => scan,
            Err(e) => {
                let reason = format!("invalid JSON response: {e}");
                self.guard_unavailable(PHASE_SCAN, reason.clone())?;
                return Ok(GuardedRequest {
                    request,
                    context: GuardContext {
                        privacy_mode: Some(privacy_mode_as_str(self.mode).to_string()),
                        privacy_failure_reason: Some(reason),
                        ..GuardContext::default()
                    },
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

            return Err(AppError::guard_contract_violation(
                "request blocked by VCAL Privacy Guard",
            ));
        }

        if scan.modified {
            if let Err(error) =
                validate_privacy_message_contract(PHASE_SCAN, &messages, &scan.messages)
            {
                metrics::GUARD_HOOK_ERRORS_TOTAL
                    .with_label_values(&[GUARD_NAME, PHASE_SCAN])
                    .inc();

                if self.fail_open {
                    tracing::warn!(
                        guard = GUARD_NAME,
                        phase = PHASE_SCAN,
                        error = %error,
                        "Privacy Guard scan contract violation; guard_fail_open=true so request continues unchanged"
                    );

                    return Ok(GuardedRequest {
                        request,
                        context: GuardContext {
                            privacy_mode: Some(privacy_mode_as_str(self.mode).to_string()),
                            privacy_failure_reason: Some(error),
                            ..GuardContext::default()
                        },
                        cache_control: CacheControl::default(),
                    });
                }

                return Err(AppError::guard_contract_violation(error));
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
                privacy_findings: scan
                    .findings
                    .iter()
                    .map(|finding| crate::evidence::DataFinding {
                        kind: finding.kind.as_str().to_string(),
                        count: finding.count as u64,
                        action: Some(scan.action.clone()),
                        detector_id: None,
                    })
                    .collect(),
                privacy_action: Some(scan.action.clone()),
                privacy_mode: Some(privacy_mode_as_str(self.mode).to_string()),
                privacy_modified: scan.modified,
                privacy_scan_skipped: false,
                privacy_failure_reason: None,
            },
            cache_control: CacheControl::default(),
        })
    }

    async fn restore_response(
        &self,
        context: &GuardContext,
        mut response: ChatCompletionResponse,
        _trace_id: Uuid,
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
                metrics::GUARD_HOOK_ERRORS_TOTAL
                    .with_label_values(&[GUARD_NAME, PHASE_RESTORE])
                    .inc();

                return Err(AppError::guard_contract_violation(error));
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

    #[tokio::test]
    async fn scan_transport_failure_fails_open_when_configured() {
        let guard = PrivacyGuardOrchestrator::new(
            "http://127.0.0.1:9".to_string(),
            None,
            PrivacyGuardMode::Anonymize,
            true,
            None,
            None,
            true,
            Duration::from_secs(1),
        );

        let result = guard
            .before_cache(openai_request_with_mixed_content(), uuid::Uuid::new_v4())
            .await;
        assert!(
            result.is_ok(),
            "Privacy Guard outage must fail open when configured"
        );
    }

    #[tokio::test]
    async fn scan_transport_failure_fails_closed_when_configured() {
        let guard = PrivacyGuardOrchestrator::new(
            "http://127.0.0.1:9".to_string(),
            None,
            PrivacyGuardMode::Anonymize,
            true,
            None,
            None,
            false,
            Duration::from_secs(1),
        );

        let error = guard
            .before_cache(openai_request_with_mixed_content(), uuid::Uuid::new_v4())
            .await
            .expect_err("Privacy Guard outage must fail closed when configured");
        assert_eq!(error.metrics_class(), "privacy_anonymization_failed");
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

        assert_eq!(error.metrics_class(), "privacy_restore_failed");
        assert_eq!(error.status_code(), axum::http::StatusCode::BAD_GATEWAY);
        assert!(error
            .message()
            .contains("Privacy Guard restore failed; response was not returned"));
        assert!(error.message().contains("simulated restore outage"));
    }

    use crate::guards::GuardContext;
    use crate::types::openai::{
        ChatCompletionRequest, ChatCompletionResponse, ChatMessage, Choice, Usage,
    };
    use serde_json::{json, Map};

    fn openai_request_with_mixed_content() -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "gpt-4o-mini-2024-07-18".to_string(),
            messages: vec![
                ChatMessage {
                    role: "user".to_string(),
                    content: json!("Email john@example.com"),
                    name: None,
                    extra: Map::new(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: json!([
                        {
                            "type": "text",
                            "text": "This non-string content should remain unchanged"
                        },
                        {
                            "type": "image_url",
                            "image_url": {
                                "url": "https://example.com/image.png"
                            }
                        }
                    ]),
                    name: None,
                    extra: Map::new(),
                },
                ChatMessage {
                    role: "system".to_string(),
                    content: json!({
                        "type": "structured",
                        "value": "leave unchanged"
                    }),
                    name: None,
                    extra: Map::new(),
                },
            ],
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        }
    }

    fn openai_response_with_mixed_content() -> ChatCompletionResponse {
        ChatCompletionResponse {
            id: "chatcmpl-test".to_string(),
            object: "chat.completion".to_string(),
            created: 1_711_111_111,
            model: "gpt-4o-mini-2024-07-18".to_string(),
            choices: vec![
                Choice {
                    index: 0,
                    message: ChatMessage {
                        role: "assistant".to_string(),
                        content: json!("Response for [EMAIL_1]"),
                        name: None,
                        extra: Map::new(),
                    },
                    finish_reason: Some("stop".to_string()),
                    extra: Map::new(),
                },
                Choice {
                    index: 1,
                    message: ChatMessage {
                        role: "assistant".to_string(),
                        content: json!([
                            {
                                "type": "text",
                                "text": "non-string assistant content"
                            }
                        ]),
                        name: None,
                        extra: Map::new(),
                    },
                    finish_reason: Some("stop".to_string()),
                    extra: Map::new(),
                },
            ],
            usage: Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                extra: Map::new(),
            }),
            extra: Map::new(),
        }
    }

    #[test]
    fn request_collection_skips_non_string_content() {
        let request = openai_request_with_mixed_content();

        let (messages, indexes) = collect_request_string_messages(&request);

        assert_eq!(messages.len(), 1);
        assert_eq!(indexes, vec![0]);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "Email john@example.com");
    }

    #[test]
    fn request_apply_replaces_only_string_message_content() {
        let mut request = openai_request_with_mixed_content();

        let original_non_string_array = request.messages[1].content.clone();
        let original_non_string_object = request.messages[2].content.clone();

        let (_messages, indexes) = collect_request_string_messages(&request);

        let anonymized = vec![PrivacyChatMessage {
            role: "user".to_string(),
            content: "Email [EMAIL_1]".to_string(),
        }];

        apply_request_string_messages(&mut request, &indexes, &anonymized);

        assert_eq!(request.messages[0].content, json!("Email [EMAIL_1]"));
        assert_eq!(
            request.messages[1].content, original_non_string_array,
            "array content must remain unchanged"
        );
        assert_eq!(
            request.messages[2].content, original_non_string_object,
            "object content must remain unchanged"
        );
    }

    #[test]
    fn response_collection_skips_non_string_content() {
        let response = openai_response_with_mixed_content();

        let (messages, indexes) = collect_response_string_messages(&response);

        assert_eq!(messages.len(), 1);
        assert_eq!(indexes, vec![0]);
        assert_eq!(messages[0].role, "assistant");
        assert_eq!(messages[0].content, "Response for [EMAIL_1]");
    }

    #[test]
    fn response_apply_replaces_only_string_message_content() {
        let mut response = openai_response_with_mixed_content();

        let original_non_string_content = response.choices[1].message.content.clone();

        let (_messages, indexes) = collect_response_string_messages(&response);

        let restored = vec![PrivacyChatMessage {
            role: "assistant".to_string(),
            content: "Response for john@example.com".to_string(),
        }];

        apply_response_string_messages(&mut response, &indexes, &restored);

        assert_eq!(
            response.choices[0].message.content,
            json!("Response for john@example.com")
        );
        assert_eq!(
            response.choices[1].message.content, original_non_string_content,
            "non-string assistant content must remain unchanged"
        );
    }

    #[tokio::test]
    async fn restore_response_without_mapping_id_is_noop() {
        let guard = PrivacyGuardOrchestrator::new(
            "http://127.0.0.1:8090".to_string(),
            None,
            PrivacyGuardMode::Anonymize,
            true,
            None,
            None,
            false,
            Duration::from_secs(1),
        );

        let response = openai_response_with_mixed_content();

        let restored = guard
            .restore_response(&GuardContext::default(), response.clone(), Uuid::new_v4())
            .await
            .unwrap();

        assert_eq!(
            restored.choices[0].message.content,
            response.choices[0].message.content
        );
        assert_eq!(
            restored.choices[1].message.content,
            response.choices[1].message.content
        );
    }
}
