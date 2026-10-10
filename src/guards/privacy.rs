use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, time::Duration};
use uuid::Uuid;

use crate::{
    config::PrivacyGuardMode,
    core::hashing::sha256_hex,
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
        ("SSN", 0usize),
        ("IBAN", 0usize),
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
        "SSN",
        "IBAN",
        "OTHER",
    ]
    .iter()
    .map(|key| format!("{}:{}", key, counts.get(*key).copied().unwrap_or(0)))
    .collect::<Vec<_>>()
    .join("|")
}

fn privacy_cache_scope_digest(
    mode: PrivacyGuardMode,
    tenant_id: Option<&str>,
    policy_id: Option<&str>,
    policy_version: Option<&str>,
    policy_hash: Option<&str>,
) -> String {
    let identity = format!(
        "privacy-cache-scope:v1\0mode={}\0tenant={}\0policy_id={}\0policy_version={}\0policy_hash={}",
        privacy_mode_as_str(mode),
        tenant_id.unwrap_or("<none>"),
        policy_id.unwrap_or("<none>"),
        policy_version.unwrap_or("<none>"),
        policy_hash.unwrap_or("<none>"),
    );
    sha256_hex(&identity)
}

fn privacy_cache_scope_for_effective_policy(
    mode: PrivacyGuardMode,
    tenant_id: Option<&str>,
    policy_id: Option<&str>,
    policy_version: Option<&str>,
    policy_hash: Option<&str>,
) -> Option<String> {
    let (Some(policy_id), Some(policy_version), Some(policy_hash)) =
        (policy_id, policy_version, policy_hash)
    else {
        return None;
    };

    Some(privacy_cache_scope_digest(
        mode,
        tenant_id,
        Some(policy_id),
        Some(policy_version),
        Some(policy_hash),
    ))
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

        let (messages, indexes, structured_text_parts) = collect_request_privacy_messages(&request)
            .map_err(|error| {
                metrics::GUARD_HOOK_ERRORS_TOTAL
                    .with_label_values(&[GUARD_NAME, PHASE_SCAN])
                    .inc();
                AppError::guard_contract_violation(error)
            })?;
        if messages.is_empty() {
            metrics::GUARD_HOOK_CALLS_TOTAL
                .with_label_values(&[GUARD_NAME, PHASE_SCAN, "skipped_no_string_content"])
                .inc();
            return Ok(GuardedRequest {
                request,
                context: GuardContext {
                    privacy_tenant_id: self.tenant_id.clone(),
                    privacy_policy_id: self.policy_id.clone(),
                    privacy_cache_scope: Some(privacy_cache_scope_digest(
                        self.mode,
                        self.tenant_id.as_deref(),
                        self.policy_id.as_deref(),
                        None,
                        None,
                    )),
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
                    cache_control: CacheControl {
                        bypass_lookup: true,
                        bypass_store: true,
                    },
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
                cache_control: CacheControl {
                    bypass_lookup: true,
                    bypass_store: true,
                },
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
                    cache_control: CacheControl {
                        bypass_lookup: true,
                        bypass_store: true,
                    },
                });
            }
        };

        if structured_text_parts > 0 {
            let coverage = scan.coverage.as_ref().ok_or_else(|| {
                AppError::guard_contract_violation(
                    "Privacy Guard scan response omitted coverage for structured text content",
                )
            })?;

            if !coverage.complete || coverage.inspected_text_parts < structured_text_parts {
                return Err(AppError::guard_contract_violation(format!(
                    "Privacy Guard scan coverage incomplete: inspected_text_parts={}, expected_at_least={}, uninspected_parts={}, complete={}",
                    coverage.inspected_text_parts,
                    structured_text_parts,
                    coverage.uninspected_parts,
                    coverage.complete
                )));
            }
        }

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
                        cache_control: CacheControl {
                            bypass_lookup: true,
                            bypass_store: true,
                        },
                    });
                }

                return Err(AppError::guard_contract_violation(error));
            }

            apply_request_privacy_messages(&mut request, &indexes, &scan.messages);
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

        let effective_tenant_id = scan.tenant_id.clone().or_else(|| self.tenant_id.clone());
        let effective_policy_id = scan.policy_id.clone().or_else(|| self.policy_id.clone());
        let privacy_cache_scope = privacy_cache_scope_for_effective_policy(
            self.mode,
            effective_tenant_id.as_deref(),
            effective_policy_id.as_deref(),
            scan.policy_version.as_deref(),
            scan.policy_hash.as_deref(),
        );
        let cache_control = if privacy_cache_scope.is_some() {
            CacheControl::default()
        } else {
            tracing::warn!(
                guard = GUARD_NAME,
                "Privacy Guard response omitted effective policy ID/version/hash; bypassing cache lookup and store for this request"
            );
            CacheControl {
                bypass_lookup: true,
                bypass_store: true,
            }
        };

        Ok(GuardedRequest {
            request,
            context: GuardContext {
                privacy_mapping_id: scan.mapping_id,
                privacy_tenant_id: effective_tenant_id,
                privacy_policy_id: effective_policy_id,
                privacy_policy_version: scan.policy_version,
                privacy_policy_hash: scan.policy_hash,
                privacy_cache_scope,
                privacy_inspected_text_parts: scan
                    .coverage
                    .as_ref()
                    .map(|coverage| coverage.inspected_text_parts as u64),
                privacy_uninspected_parts: scan
                    .coverage
                    .as_ref()
                    .map(|coverage| coverage.uninspected_parts as u64),
                privacy_inspection_complete: scan
                    .coverage
                    .as_ref()
                    .map(|coverage| coverage.complete),
                privacy_placeholder_signature,
                privacy_findings: scan
                    .findings
                    .iter()
                    .map(|finding| crate::evidence::DataFinding {
                        kind: finding.kind.as_str().to_string(),
                        count: finding.count as u64,
                        action: finding.action.clone().or_else(|| {
                            (!scan.action.eq_ignore_ascii_case("mixed"))
                                .then(|| scan.action.clone())
                        }),
                        detector_id: finding.detector_id.clone(),
                    })
                    .collect(),
                privacy_action: Some(scan.action.clone()),
                privacy_mode: Some(privacy_mode_as_str(self.mode).to_string()),
                privacy_modified: scan.modified,
                privacy_scan_skipped: false,
                privacy_failure_reason: None,
                ..GuardContext::default()
            },
            cache_control,
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

        let (messages, indexes, _structured_text_parts) =
            collect_response_privacy_messages(&response).map_err(|error| {
                metrics::GUARD_HOOK_ERRORS_TOTAL
                    .with_label_values(&[GUARD_NAME, PHASE_RESTORE])
                    .inc();
                AppError::guard_contract_violation(error)
            })?;
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

            apply_response_privacy_messages(&mut response, &indexes, &restored.messages);
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
            "Privacy Guard {phase} response contract violation: expected {} messages, got {}",
            expected.len(),
            returned.len()
        ));
    }

    for (position, (expected_message, returned_message)) in
        expected.iter().zip(returned.iter()).enumerate()
    {
        if expected_message.role != returned_message.role {
            return Err(format!(
                "Privacy Guard {phase} response contract violation: role mismatch at message position {position}: expected role '{}', got '{}'",
                expected_message.role,
                returned_message.role
            ));
        }

        validate_privacy_content_contract(
            phase,
            &expected_message.content,
            &returned_message.content,
            &format!("messages[{position}].content"),
        )?;
    }

    Ok(())
}

fn validate_privacy_content_contract(
    phase: &'static str,
    expected: &Value,
    returned: &Value,
    path: &str,
) -> Result<(), String> {
    match (expected, returned) {
        (Value::String(_), Value::String(_)) => Ok(()),
        (Value::Array(expected_items), Value::Array(returned_items)) => {
            if expected_items.len() != returned_items.len() {
                return Err(format!(
                    "Privacy Guard {phase} response contract violation: array length changed at {path}: expected {}, got {}",
                    expected_items.len(),
                    returned_items.len()
                ));
            }

            for (index, (expected_item, returned_item)) in
                expected_items.iter().zip(returned_items.iter()).enumerate()
            {
                let item_path = format!("{path}[{index}]");
                if is_typed_text_part(expected_item) {
                    validate_typed_text_part(phase, expected_item, returned_item, &item_path)?;
                } else if matches!(expected_item, Value::String(_)) {
                    if !matches!(returned_item, Value::String(_)) {
                        return Err(format!(
                            "Privacy Guard {phase} response contract violation: text string shape changed at {item_path}"
                        ));
                    }
                } else if expected_item != returned_item {
                    return Err(format!(
                        "Privacy Guard {phase} response contract violation: non-text structured content changed at {item_path}"
                    ));
                }
            }

            Ok(())
        }
        _ if expected == returned => Ok(()),
        _ => Err(format!(
            "Privacy Guard {phase} response contract violation: content shape changed at {path}"
        )),
    }
}

fn validate_typed_text_part(
    phase: &'static str,
    expected: &Value,
    returned: &Value,
    path: &str,
) -> Result<(), String> {
    let Some(expected_object) = expected.as_object() else {
        return Err(format!(
            "Privacy Guard {phase} response contract violation: expected typed text object at {path}"
        ));
    };
    let Some(returned_object) = returned.as_object() else {
        return Err(format!(
            "Privacy Guard {phase} response contract violation: typed text object shape changed at {path}"
        ));
    };

    if !returned_object.get("text").is_some_and(Value::is_string) {
        return Err(format!(
            "Privacy Guard {phase} response contract violation: typed text field is not a string at {path}.text"
        ));
    }

    let mut expected_non_text = expected_object.clone();
    expected_non_text.remove("text");
    let mut returned_non_text = returned_object.clone();
    returned_non_text.remove("text");

    if expected_non_text != returned_non_text {
        return Err(format!(
            "Privacy Guard {phase} response contract violation: non-text fields changed at {path}"
        ));
    }

    Ok(())
}

fn collect_request_privacy_messages(
    request: &ChatCompletionRequest,
) -> Result<(Vec<PrivacyChatMessage>, Vec<usize>, usize), String> {
    collect_privacy_messages(
        request
            .messages
            .iter()
            .enumerate()
            .map(|(index, message)| (index, message.role.as_str(), &message.content)),
        PHASE_SCAN,
    )
}

fn collect_response_privacy_messages(
    response: &ChatCompletionResponse,
) -> Result<(Vec<PrivacyChatMessage>, Vec<usize>, usize), String> {
    collect_privacy_messages(
        response
            .choices
            .iter()
            .enumerate()
            .map(|(index, choice)| (index, choice.message.role.as_str(), &choice.message.content)),
        PHASE_RESTORE,
    )
}

fn collect_privacy_messages<'a, I>(
    messages: I,
    phase: &'static str,
) -> Result<(Vec<PrivacyChatMessage>, Vec<usize>, usize), String>
where
    I: IntoIterator<Item = (usize, &'a str, &'a Value)>,
{
    let mut collected = Vec::new();
    let mut indexes = Vec::new();
    let mut structured_text_parts = 0usize;

    for (index, role, content) in messages {
        let inspection = inspect_content_shape(content);
        if inspection.unsupported_text_bearing {
            return Err(format!(
                "Privacy Guard {phase} cannot safely inspect unsupported text-bearing content for role '{role}'"
            ));
        }

        if inspection.text_parts == 0 {
            continue;
        }

        if !content.is_string() {
            metrics::GUARD_NON_STRING_CONTENT_TOTAL
                .with_label_values(&[GUARD_NAME, phase])
                .inc();
        }

        structured_text_parts += inspection.structured_text_parts;
        collected.push(PrivacyChatMessage {
            role: role.to_string(),
            content: content.clone(),
        });
        indexes.push(index);
    }

    Ok((collected, indexes, structured_text_parts))
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ContentInspection {
    text_parts: usize,
    structured_text_parts: usize,
    unsupported_text_bearing: bool,
}

fn inspect_content_shape(content: &Value) -> ContentInspection {
    match content {
        Value::String(_) => ContentInspection {
            text_parts: 1,
            ..ContentInspection::default()
        },
        Value::Array(items) => {
            let mut inspection = ContentInspection::default();
            for item in items {
                match item {
                    Value::String(_) => {
                        inspection.text_parts += 1;
                        inspection.structured_text_parts += 1;
                    }
                    Value::Object(object) if is_typed_text_object(object) => {
                        inspection.text_parts += 1;
                        inspection.structured_text_parts += 1;
                    }
                    Value::Object(object) if object_contains_text_bearing_field(object) => {
                        inspection.unsupported_text_bearing = true;
                    }
                    _ => {}
                }
            }
            inspection
        }
        Value::Object(object) if object_contains_text_bearing_field(object) => ContentInspection {
            unsupported_text_bearing: true,
            ..ContentInspection::default()
        },
        _ => ContentInspection::default(),
    }
}

fn is_typed_text_part(value: &Value) -> bool {
    value.as_object().is_some_and(is_typed_text_object)
}

fn is_typed_text_object(object: &Map<String, Value>) -> bool {
    object.get("type").and_then(Value::as_str) == Some("text")
        && object.get("text").is_some_and(Value::is_string)
}

fn object_contains_text_bearing_field(object: &Map<String, Value>) -> bool {
    object.iter().any(|(key, value)| {
        matches!(
            key.as_str(),
            "text" | "content" | "input_text" | "output_text"
        ) && value_contains_text(value)
            || match value {
                Value::Object(nested) => object_contains_text_bearing_field(nested),
                Value::Array(items) => items.iter().any(|item| match item {
                    Value::Object(nested) => object_contains_text_bearing_field(nested),
                    Value::Array(_) => value_contains_nested_text_bearing_field(item),
                    _ => false,
                }),
                _ => false,
            }
    })
}

fn value_contains_text(value: &Value) -> bool {
    match value {
        Value::String(_) => true,
        Value::Array(items) => items.iter().any(value_contains_text),
        Value::Object(object) => object.values().any(value_contains_text),
        _ => false,
    }
}

fn value_contains_nested_text_bearing_field(value: &Value) -> bool {
    match value {
        Value::Object(object) => object_contains_text_bearing_field(object),
        Value::Array(items) => items.iter().any(value_contains_nested_text_bearing_field),
        _ => false,
    }
}

fn apply_request_privacy_messages(
    request: &mut ChatCompletionRequest,
    indexes: &[usize],
    messages: &[PrivacyChatMessage],
) {
    for (idx, message) in indexes.iter().copied().zip(messages.iter()) {
        if let Some(target) = request.messages.get_mut(idx) {
            target.content = message.content.clone();
        }
    }
}

fn apply_response_privacy_messages(
    response: &mut ChatCompletionResponse,
    indexes: &[usize],
    messages: &[PrivacyChatMessage],
) {
    for (idx, message) in indexes.iter().copied().zip(messages.iter()) {
        if let Some(choice) = response.choices.get_mut(idx) {
            choice.message.content = message.content.clone();
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
    content: Value,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct PrivacyScanResponse {
    request_id: String,
    tenant_id: Option<String>,
    policy_id: Option<String>,
    #[serde(default)]
    policy_version: Option<String>,
    #[serde(default)]
    policy_hash: Option<String>,
    decision: String,
    action: String,
    modified: bool,
    mapping_id: Option<String>,
    messages: Vec<PrivacyChatMessage>,
    findings: Vec<PrivacyFinding>,
    #[serde(default)]
    coverage: Option<PrivacyInspectionCoverage>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PrivacyInspectionCoverage {
    inspected_text_parts: usize,
    uninspected_parts: usize,
    complete: bool,
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
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    detector_id: Option<String>,
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
    Ssn,
    Iban,
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
            Self::Ssn => "ssn",
            Self::Iban => "iban",
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
            Self::Ssn => "SSN",
            Self::Iban => "IBAN",
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
            content: Value::String(content.to_string()),
        }
    }

    #[test]
    fn placeholder_signature_includes_ssn_and_iban_findings() {
        let findings = vec![
            PrivacyFinding {
                kind: PrivacyFindingKind::Ssn,
                count: 1,
                severity: PrivacySeverity::High,
                action: Some("anonymize".to_string()),
                detector_id: None,
            },
            PrivacyFinding {
                kind: PrivacyFindingKind::Iban,
                count: 2,
                severity: PrivacySeverity::High,
                action: Some("anonymize".to_string()),
                detector_id: None,
            },
        ];

        assert_eq!(
            placeholder_signature_from_findings(&findings),
            "EMAIL:0|IP:0|PHONE:0|JWT:0|API_KEY:0|BEARER_TOKEN:0|PRIVATE_KEY:0|CREDIT_CARD_LIKE:0|SSN:1|IBAN:2|OTHER:0"
        );
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

        assert!(error.contains("expected 2 messages, got 1"));
    }

    #[test]
    fn privacy_message_contract_rejects_role_mismatch() {
        let expected = vec![msg("user", "a")];
        let returned = vec![msg("assistant", "x")];

        let error = validate_privacy_message_contract(PHASE_RESTORE, &expected, &returned)
            .expect_err("role mismatch must be rejected");

        assert!(error.contains("role mismatch at message position 0"));
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
            .await
            .expect("Privacy Guard outage must fail open when configured");
        assert!(result.cache_control.bypass_lookup);
        assert!(result.cache_control.bypass_store);
        assert!(result.context.privacy_failure_reason.is_some());
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
    fn request_collection_includes_supported_structured_text() {
        let request = openai_request_with_mixed_content();

        let (messages, indexes, structured_text_parts) =
            collect_request_privacy_messages(&request).unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(indexes, vec![0, 1]);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, json!("Email john@example.com"));
        assert_eq!(messages[1].content, request.messages[1].content);
        assert_eq!(structured_text_parts, 1);
    }

    #[test]
    fn request_apply_preserves_structured_shape_and_non_text_parts() {
        let mut request = openai_request_with_mixed_content();
        let original_image_part = request.messages[1].content[1].clone();
        let original_non_text_object = request.messages[2].content.clone();

        let (messages, indexes, _) = collect_request_privacy_messages(&request).unwrap();
        let mut anonymized = messages.clone();
        anonymized[0].content = json!("Email [EMAIL_1]");
        anonymized[1].content[0]["text"] = json!("Contact [EMAIL_1]");

        validate_privacy_message_contract(PHASE_SCAN, &messages, &anonymized).unwrap();
        apply_request_privacy_messages(&mut request, &indexes, &anonymized);

        assert_eq!(request.messages[0].content, json!("Email [EMAIL_1]"));
        assert_eq!(
            request.messages[1].content[0],
            json!({"type": "text", "text": "Contact [EMAIL_1]"})
        );
        assert_eq!(request.messages[1].content[1], original_image_part);
        assert_eq!(request.messages[2].content, original_non_text_object);
    }

    #[test]
    fn request_collection_rejects_unsupported_text_bearing_shape() {
        let mut request = openai_request_with_mixed_content();
        request.messages[2].content = json!({
            "type": "structured",
            "text": "Email john@example.com"
        });

        let error = collect_request_privacy_messages(&request)
            .expect_err("unsupported text-bearing content must not bypass Privacy Guard");

        assert!(error.contains("cannot safely inspect unsupported text-bearing content"));
    }

    #[test]
    fn request_collection_rejects_nested_unsupported_text_bearing_shape() {
        let mut request = openai_request_with_mixed_content();
        request.messages[2].content = json!({
            "type": "vendor_extension",
            "payload": {
                "content": [{"text": "Email john@example.com"}]
            }
        });

        let error = collect_request_privacy_messages(&request)
            .expect_err("nested unsupported text-bearing content must not bypass Privacy Guard");

        assert!(error.contains("cannot safely inspect unsupported text-bearing content"));
    }

    #[test]
    fn privacy_cache_scope_digest_binds_tenant_and_policy_metadata_without_exposing_it() {
        let base = privacy_cache_scope_digest(
            PrivacyGuardMode::Anonymize,
            Some("tenant-a"),
            Some("policy-a"),
            Some("3"),
            Some("sha256:abc"),
        );
        let other_tenant = privacy_cache_scope_digest(
            PrivacyGuardMode::Anonymize,
            Some("tenant-b"),
            Some("policy-a"),
            Some("3"),
            Some("sha256:abc"),
        );
        let other_policy = privacy_cache_scope_digest(
            PrivacyGuardMode::Anonymize,
            Some("tenant-a"),
            Some("policy-b"),
            Some("4"),
            Some("sha256:def"),
        );

        assert_eq!(base.len(), 64);
        assert!(base.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(base, other_tenant);
        assert_ne!(base, other_policy);
        assert!(!base.contains("tenant-a"));
        assert!(!base.contains("policy-a"));
    }

    #[test]
    fn privacy_cache_scope_requires_policy_id_version_and_hash() {
        assert!(privacy_cache_scope_for_effective_policy(
            PrivacyGuardMode::Anonymize,
            Some("tenant-a"),
            Some("policy-a"),
            Some("3"),
            Some("sha256:abc"),
        )
        .is_some());
        assert!(privacy_cache_scope_for_effective_policy(
            PrivacyGuardMode::Anonymize,
            Some("tenant-a"),
            Some("policy-a"),
            None,
            Some("sha256:abc"),
        )
        .is_none());
        assert!(privacy_cache_scope_for_effective_policy(
            PrivacyGuardMode::Anonymize,
            Some("tenant-a"),
            Some("policy-a"),
            Some("3"),
            None,
        )
        .is_none());
    }

    #[test]
    fn privacy_contract_rejects_non_text_part_mutation() {
        let expected = vec![PrivacyChatMessage {
            role: "user".to_string(),
            content: json!([
                {"type": "text", "text": "Email john@example.com"},
                {"type": "image_url", "image_url": {"url": "https://example.com/a.png"}}
            ]),
        }];
        let returned = vec![PrivacyChatMessage {
            role: "user".to_string(),
            content: json!([
                {"type": "text", "text": "Email [EMAIL_1]"},
                {"type": "image_url", "image_url": {"url": "https://example.com/b.png"}}
            ]),
        }];

        let error = validate_privacy_message_contract(PHASE_SCAN, &expected, &returned)
            .expect_err("Privacy Guard must not rewrite non-text structured parts");
        assert!(error.contains("non-text structured content changed"));
    }

    #[test]
    fn response_collection_and_apply_support_structured_text() {
        let mut response = openai_response_with_mixed_content();
        let original_first = response.choices[0].message.content.clone();

        let (messages, indexes, structured_text_parts) =
            collect_response_privacy_messages(&response).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(indexes, vec![0, 1]);
        assert_eq!(structured_text_parts, 1);

        let mut restored = messages.clone();
        restored[0].content = json!("Response for john@example.com");
        restored[1].content[0]["text"] = json!("Response for john@example.com");
        validate_privacy_message_contract(PHASE_RESTORE, &messages, &restored).unwrap();
        apply_response_privacy_messages(&mut response, &indexes, &restored);

        assert_ne!(response.choices[0].message.content, original_first);
        assert_eq!(
            response.choices[0].message.content,
            json!("Response for john@example.com")
        );
        assert_eq!(
            response.choices[1].message.content[0],
            json!({"type": "text", "text": "Response for john@example.com"})
        );
    }

    #[test]
    fn v030_scan_response_deserializes_policy_coverage_and_finding_metadata() {
        let value = json!({
            "request_id": "req-1",
            "tenant_id": "tenant-a",
            "policy_id": "enterprise-default",
            "policy_version": "3",
            "policy_hash": "sha256:abc123",
            "decision": "allow",
            "action": "mixed",
            "modified": true,
            "mapping_id": "map-1",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "Email [EMAIL_1]"},
                    {"type": "image_url", "image_url": {"url": "https://example.com/a.png"}}
                ]
            }],
            "findings": [{
                "kind": "email",
                "count": 1,
                "severity": "high",
                "action": "anonymize",
                "detector_id": "corp-email"
            }],
            "coverage": {
                "inspected_text_parts": 1,
                "uninspected_parts": 0,
                "complete": true
            },
            "warnings": []
        });

        let scan: PrivacyScanResponse = serde_json::from_value(value).unwrap();
        assert_eq!(scan.policy_id.as_deref(), Some("enterprise-default"));
        assert_eq!(scan.policy_version.as_deref(), Some("3"));
        assert_eq!(scan.policy_hash.as_deref(), Some("sha256:abc123"));
        assert_eq!(scan.action, "mixed");
        assert_eq!(scan.findings[0].action.as_deref(), Some("anonymize"));
        assert_eq!(scan.findings[0].detector_id.as_deref(), Some("corp-email"));
        let coverage = scan.coverage.expect("v0.3.0 coverage must deserialize");
        assert_eq!(coverage.inspected_text_parts, 1);
        assert_eq!(coverage.uninspected_parts, 0);
        assert!(coverage.complete);
    }

    #[tokio::test]
    async fn structured_scan_applies_v030_response_and_captures_effective_metadata() {
        use axum::{routing::post, Json, Router};

        let app = Router::new().route(
            "/v1/scan",
            post(|| async {
                Json(json!({
                    "request_id": "req-structured",
                    "tenant_id": "tenant-effective",
                    "policy_id": "enterprise-default",
                    "policy_version": "3",
                    "policy_hash": "sha256:abc123",
                    "decision": "allow",
                    "action": "anonymize",
                    "modified": true,
                    "mapping_id": "map-structured",
                    "messages": [
                        {
                            "role": "user",
                            "content": "Email [EMAIL_1]"
                        },
                        {
                            "role": "user",
                            "content": [
                                {"type": "text", "text": "Contact [EMAIL_1]"},
                                {
                                    "type": "image_url",
                                    "image_url": {"url": "https://example.com/image.png"}
                                }
                            ]
                        }
                    ],
                    "findings": [{
                        "kind": "email",
                        "count": 1,
                        "severity": "high",
                        "action": "anonymize",
                        "detector_id": "corp-email"
                    }],
                    "coverage": {
                        "inspected_text_parts": 2,
                        "uninspected_parts": 1,
                        "complete": true
                    },
                    "warnings": []
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let guard = PrivacyGuardOrchestrator::new(
            format!("http://{addr}"),
            None,
            PrivacyGuardMode::Anonymize,
            true,
            Some("tenant-requested".to_string()),
            Some("policy-requested".to_string()),
            false,
            Duration::from_secs(2),
        );

        let original = openai_request_with_mixed_content();
        let original_image = original.messages[1].content[1].clone();
        let guarded = guard
            .before_cache(original, uuid::Uuid::new_v4())
            .await
            .unwrap();
        server.abort();

        assert_eq!(
            guarded.request.messages[0].content,
            json!("Email [EMAIL_1]")
        );
        assert_eq!(
            guarded.request.messages[1].content[0],
            json!({"type": "text", "text": "Contact [EMAIL_1]"})
        );
        assert_eq!(guarded.request.messages[1].content[1], original_image);
        assert_eq!(
            guarded.context.privacy_tenant_id.as_deref(),
            Some("tenant-effective")
        );
        assert_eq!(
            guarded.context.privacy_policy_id.as_deref(),
            Some("enterprise-default")
        );
        assert_eq!(guarded.context.privacy_policy_version.as_deref(), Some("3"));
        assert_eq!(
            guarded.context.privacy_policy_hash.as_deref(),
            Some("sha256:abc123")
        );
        assert_eq!(guarded.context.privacy_inspected_text_parts, Some(2));
        assert_eq!(guarded.context.privacy_uninspected_parts, Some(1));
        assert_eq!(guarded.context.privacy_inspection_complete, Some(true));
        assert_eq!(
            guarded.context.privacy_findings[0].detector_id.as_deref(),
            Some("corp-email")
        );
        assert_eq!(
            guarded.context.privacy_findings[0].action.as_deref(),
            Some("anonymize")
        );
        assert_eq!(
            guarded.context.privacy_cache_scope.as_deref().map(str::len),
            Some(64)
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
