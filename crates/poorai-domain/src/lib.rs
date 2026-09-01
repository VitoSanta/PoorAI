//! Versioned, provider-independent poorAI domain contracts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;

pub type Id = Uuid;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("invalid {field}: {reason}")]
    Invalid { field: &'static str, reason: String },
    #[error("incompatible {left} and {right}")]
    Incompatible {
        left: &'static str,
        right: &'static str,
    },
}

pub trait Validate {
    fn validate(&self) -> Result<(), DomainError>;
}

pub fn new_id() -> Id {
    Uuid::now_v7()
}
pub fn now() -> DateTime<Utc> {
    Utc::now()
}
pub fn hash_bytes(value: impl AsRef<[u8]>) -> String {
    blake3::hash(value.as_ref()).to_hex().to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Provenance {
    pub source: String,
    pub observed_at: DateTime<Utc>,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelDefinition {
    pub schema_version: u32,
    pub id: Id,
    pub digest: String,
    pub family: Option<String>,
    pub quantization: Option<String>,
    pub capabilities: BTreeMap<String, Observation>,
    pub metadata: serde_json::Value,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum Observation {
    Observed(serde_json::Value),
    Unknown { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelStrategy {
    pub schema_version: u32,
    pub id: Id,
    pub model_selector: String,
    pub role: String,
    pub prompting: serde_json::Value,
    pub reasoning: serde_json::Value,
    pub tool_policy: String,
    pub retrieval_policy: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeploymentDescriptor {
    pub schema_version: u32,
    pub id: Id,
    pub provider: String,
    pub endpoint: String,
    pub model_ref: String,
    pub backend_options: BTreeMap<String, String>,
    pub auth_ref: Option<String>,
}
impl DeploymentDescriptor {
    pub fn fingerprint(&self) -> String {
        let safe = serde_json::json!({"provider": self.provider, "endpoint": self.endpoint, "model_ref": self.model_ref, "backend_options": self.backend_options});
        hash_bytes(serde_json::to_vec(&safe).expect("JSON serializable"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HardwareProfile {
    pub schema_version: u32,
    pub id: Id,
    pub compatibility_key: String,
    pub os: String,
    pub architecture: String,
    pub cpu: String,
    pub accelerators: Vec<String>,
    pub total_memory_bytes: Option<u64>,
    pub storage_free_bytes: Option<u64>,
    pub unavailable_fields: Vec<String>,
    pub probe_version: String,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    pub schema_version: u32,
    pub id: Id,
    pub hardware_id: Id,
    pub deployment_id: Id,
    pub timestamp: DateTime<Utc>,
    pub available_memory_bytes: Option<u64>,
    pub pressure: Observation,
    pub loaded_models: Vec<String>,
    pub backend_state: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StablePoint {
    pub context_tokens: u32,
    pub samples: u32,
    pub success_rate: f64,
    pub median_first_token_ms: f64,
    pub generation_tokens_per_second: f64,
    pub variance: f64,
    pub memory_pressure_observed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CalibrationProfile {
    pub schema_version: u32,
    pub id: Id,
    pub compatibility_key: String,
    pub model_digest: String,
    pub deployment_fingerprint: String,
    pub harness_rev: String,
    pub stable_points: Vec<StablePoint>,
    pub raw_artifact_hashes: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EvidenceLabel {
    Measured,
    ConservativeBootstrap,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionProfile {
    pub schema_version: u32,
    pub id: Id,
    pub strategy_id: Id,
    pub calibration_id: Option<Id>,
    pub context_tokens: u32,
    pub reserve_tokens: u32,
    pub concurrency: u16,
    pub budgets: serde_json::Value,
    pub rationale: String,
    pub evidence: EvidenceLabel,
    pub compatibility_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvaluationRun {
    pub schema_version: u32,
    pub id: Id,
    pub corpus_rev: String,
    pub task_set: String,
    pub execution_profile_id: Id,
    pub model_digest: String,
    pub deployment_fingerprint: String,
    pub hardware_compatibility_key: String,
    pub harness_rev: String,
    pub seeds: Vec<u64>,
    pub outcome_hash: String,
    pub artifact_hashes: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInspection {
    pub definition: ModelDefinition,
    pub deployment: DeploymentDescriptor,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackendState {
    pub observed_at: DateTime<Utc>,
    pub loaded_models: Vec<String>,
    pub state: serde_json::Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRequest {
    pub deployment: DeploymentDescriptor,
    pub messages: Vec<ChatMessage>,
    pub context_tokens: u32,
    pub tools: Option<serde_json::Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}
/// Provider-neutral tool invocation requested by a model.
///
/// Kept structural: adapters never flatten a call into prose, and callers never
/// re-parse `content` to guess that a call happened.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelChunk {
    pub content: String,
    /// Reasoning text when the deployment emits it on a separate channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    pub done: bool,
}

impl ModelChunk {
    /// A chunk carries no observable payload when every channel is empty.
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
            && self.thinking.as_ref().is_none_or(|t| t.is_empty())
            && self.tool_calls.is_empty()
    }
}

impl Validate for DeploymentDescriptor {
    fn validate(&self) -> Result<(), DomainError> {
        if self.provider.is_empty() || self.model_ref.is_empty() {
            return Err(DomainError::Invalid {
                field: "deployment",
                reason: "provider and model_ref are required".into(),
            });
        }
        if !self.endpoint.starts_with("http://") && !self.endpoint.starts_with("https://") {
            return Err(DomainError::Invalid {
                field: "endpoint",
                reason: "must be an HTTP(S) URL".into(),
            });
        }
        Ok(())
    }
}
impl Validate for CalibrationProfile {
    fn validate(&self) -> Result<(), DomainError> {
        if self.model_digest.is_empty()
            || self.compatibility_key.is_empty()
            || self.harness_rev.is_empty()
        {
            return Err(DomainError::Invalid {
                field: "calibration",
                reason: "model, compatibility key, and harness revision are required".into(),
            });
        }
        if self.stable_points.iter().any(|p| {
            p.context_tokens == 0 || p.samples < 3 || !(0.0..=1.0).contains(&p.success_rate)
        }) {
            return Err(DomainError::Invalid {
                field: "stable_points",
                reason: "must have >=3 samples and valid measured success rates".into(),
            });
        }
        Ok(())
    }
}
impl ExecutionProfile {
    pub fn validate_against(
        &self,
        calibration: Option<&CalibrationProfile>,
    ) -> Result<(), DomainError> {
        if self.context_tokens == 0
            || self.reserve_tokens >= self.context_tokens
            || self.concurrency == 0
        {
            return Err(DomainError::Invalid {
                field: "execution_profile",
                reason: "invalid context, reserve, or concurrency".into(),
            });
        }
        match (self.calibration_id, calibration, &self.evidence) {
            (Some(id), Some(profile), EvidenceLabel::Measured)
                if id == profile.id
                    && self.compatibility_key == profile.compatibility_key
                    && profile
                        .stable_points
                        .iter()
                        .any(|p| p.context_tokens >= self.context_tokens) =>
            {
                Ok(())
            }
            (None, None, EvidenceLabel::ConservativeBootstrap) => Ok(()),
            _ => Err(DomainError::Incompatible {
                left: "execution profile",
                right: "calibration profile",
            }),
        }
    }
}
impl Validate for EvaluationRun {
    fn validate(&self) -> Result<(), DomainError> {
        if self.corpus_rev.is_empty()
            || self.harness_rev.is_empty()
            || self.model_digest.is_empty()
            || self.outcome_hash.is_empty()
        {
            return Err(DomainError::Invalid {
                field: "evaluation",
                reason: "corpus, harness, model, and outcome provenance are required".into(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids_are_v7() {
        assert_eq!(new_id().get_version_num(), 7);
    }
    #[test]
    fn measured_profile_rejects_bad_compatibility() {
        let p = CalibrationProfile {
            schema_version: 1,
            id: new_id(),
            compatibility_key: "a".into(),
            model_digest: "x".into(),
            deployment_fingerprint: "d".into(),
            harness_rev: "h".into(),
            stable_points: vec![StablePoint {
                context_tokens: 512,
                samples: 3,
                success_rate: 1.0,
                median_first_token_ms: 1.,
                generation_tokens_per_second: 1.,
                variance: 0.,
                memory_pressure_observed: false,
            }],
            raw_artifact_hashes: vec![],
            created_at: now(),
        };
        let e = ExecutionProfile {
            schema_version: 1,
            id: new_id(),
            strategy_id: new_id(),
            calibration_id: Some(p.id),
            context_tokens: 512,
            reserve_tokens: 1,
            concurrency: 1,
            budgets: serde_json::json!({}),
            rationale: "test".into(),
            evidence: EvidenceLabel::Measured,
            compatibility_key: "b".into(),
        };
        assert!(e.validate_against(Some(&p)).is_err());
    }
}
