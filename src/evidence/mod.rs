pub mod buffered_http;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

pub const EVIDENCE_SCHEMA_NAME: &str = "vcal.evidence.event";
pub const EVIDENCE_SCHEMA_VERSION: &str = "1.1";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    AiFirewall,
    PrivacyGuard,
    SecurityGuard,
    Audit,
    Compliance,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventCategory {
    Request,
    Privacy,
    Security,
    Cache,
    Upstream,
    Policy,
    Control,
    System,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventOutcome {
    Started,
    Allowed,
    Blocked,
    Completed,
    Failed,
    Hit,
    Miss,
    Bypassed,
    Stored,
    Skipped,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyRef {
    pub policy_id: String,
    pub policy_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_type: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActorRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DataFinding {
    pub kind: String,
    pub count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detector_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CacheEvidence {
    pub cache_type: String,
    pub operation: String,
    pub outcome: EventOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_key_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity_score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f32>,
    #[serde(default)]
    pub upstream_called: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpstreamEvidence {
    pub provider_type: String,
    pub provider_name: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DecisionEvidence {
    pub action: String,
    pub reason_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EvidenceEvent {
    pub schema: String,
    pub schema_version: String,
    pub event_id: Uuid,
    pub trace_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_event_id: Option<Uuid>,
    pub occurred_at: DateTime<Utc>,
    pub source: EvidenceSource,
    pub category: EventCategory,
    pub event_type: String,
    pub outcome: EventOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<ActorRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<PolicyRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<DataFinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<UpstreamEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<DecisionEvidence>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub attributes: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_event_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_hash: Option<String>,
}

impl EvidenceEvent {
    pub fn new(
        trace_id: Uuid,
        source: EvidenceSource,
        category: EventCategory,
        event_type: impl Into<String>,
        outcome: EventOutcome,
    ) -> Self {
        Self {
            schema: EVIDENCE_SCHEMA_NAME.to_string(),
            schema_version: EVIDENCE_SCHEMA_VERSION.to_string(),
            event_id: Uuid::new_v4(),
            trace_id,
            parent_event_id: None,
            occurred_at: Utc::now(),
            source,
            category,
            event_type: event_type.into(),
            outcome,
            actor: None,
            policy: None,
            findings: Vec::new(),
            cache: None,
            upstream: None,
            decision: None,
            attributes: Map::new(),
            previous_event_hash: None,
            event_hash: None,
        }
    }
}

#[async_trait]
pub trait EvidenceSink: Send + Sync {
    async fn emit(&self, event: EvidenceEvent) -> anyhow::Result<()>;
}

#[allow(dead_code)]
pub struct NoopEvidenceSink;

#[async_trait]
impl EvidenceSink for NoopEvidenceSink {
    async fn emit(&self, _event: EvidenceEvent) -> anyhow::Result<()> {
        Ok(())
    }
}

pub struct TracingEvidenceSink;

#[async_trait]
impl EvidenceSink for TracingEvidenceSink {
    async fn emit(&self, event: EvidenceEvent) -> anyhow::Result<()> {
        tracing::info!(
            target: "vcal_evidence",
            event = %serde_json::to_string(&event)?,
            "VCAL evidence event"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_versioned_event_without_payload_data() {
        let trace_id = Uuid::new_v4();
        let event = EvidenceEvent::new(
            trace_id,
            EvidenceSource::AiFirewall,
            EventCategory::Cache,
            "cache.semantic.lookup",
            EventOutcome::Hit,
        );

        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["schema"], EVIDENCE_SCHEMA_NAME);
        assert_eq!(value["schema_version"], EVIDENCE_SCHEMA_VERSION);
        assert_eq!(value["trace_id"], trace_id.to_string());
        assert!(value.get("prompt").is_none());
        assert!(value.get("response").is_none());
    }
}
