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

/// Policy for one deployment, as opposed to facts about it.
///
/// Measured differences between deployments are large and do not point the same
/// way: one prompt change moved one deployment by seven tasks and another by
/// none. A single-prompt harness cannot express that, so this is what a run
/// consults instead of a constant.
///
/// A strategy is a hypothesis until it is measured against the default. Nothing
/// here is evidence on its own.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelStrategy {
    pub schema_version: u32,
    pub id: Id,
    /// Matched against the deployment's model reference, exactly.
    pub model_selector: String,
    pub role: String,
    /// Appended to the shared system prompt. Empty means the shared one alone.
    #[serde(default)]
    pub prompt_suffix: String,
    /// Overrides the execution profile's action budget.
    #[serde(default)]
    pub max_actions: Option<u8>,
    /// Repository passages offered at the start.
    #[serde(default)]
    pub retrieval_excerpts: Option<usize>,
    /// Why this strategy exists, and what measurement prompted it.
    pub rationale: String,
}

impl ModelStrategy {
    /// The strategy for a deployment, if one is declared.
    pub fn select<'a>(
        strategies: &'a [ModelStrategy],
        model_ref: &str,
    ) -> Option<&'a ModelStrategy> {
        strategies
            .iter()
            .find(|strategy| strategy.model_selector == model_ref)
    }
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

/// Admission criteria a measured point must meet to count as stable.
///
/// Stored on the profile so the evidence carries the standard it was judged
/// against: a reader can check the points rather than trust them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CalibrationThresholds {
    pub min_success_rate: f64,
    pub max_median_first_token_ms: f64,
    pub allow_memory_pressure: bool,
}
impl Default for CalibrationThresholds {
    fn default() -> Self {
        Self {
            // A tier that failed any sample is not a tier to operate at.
            min_success_rate: 1.0,
            max_median_first_token_ms: 120_000.0,
            allow_memory_pressure: false,
        }
    }
}
impl CalibrationThresholds {
    pub fn admits(&self, point: &StablePoint) -> bool {
        point.success_rate >= self.min_success_rate
            && point.median_first_token_ms <= self.max_median_first_token_ms
            && (self.allow_memory_pressure || !point.memory_pressure_observed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CalibrationProfile {
    pub schema_version: u32,
    pub id: Id,
    pub compatibility_key: String,
    pub model_digest: String,
    pub deployment_fingerprint: String,
    pub harness_rev: String,
    pub thresholds: CalibrationThresholds,
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
    /// Sampling seed. A run that records a seed it never sent is not
    /// reproducible, whatever the record says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Sampling temperature, in thousandths so the request stays comparable
    /// by value. Measured on this host: a seed alone does not make sampling
    /// reproducible, and a seed with temperature 0 does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature_milli: Option<u64>,
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
/// Exact counts and timings reported by the backend for one generation.
///
/// Token counting is otherwise an estimate; where a backend reports real
/// counts, calibration uses them instead of a local proxy.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerationMetrics {
    pub prompt_tokens: Option<u64>,
    pub generated_tokens: Option<u64>,
    pub total_duration_ns: Option<u64>,
    /// Time spent loading the model. A warm deployment reports near zero, which
    /// is how a warm-up is verified rather than assumed.
    pub load_duration_ns: Option<u64>,
    pub prompt_eval_duration_ns: Option<u64>,
    pub generation_duration_ns: Option<u64>,
}
impl GenerationMetrics {
    /// Backend-reported generation rate, when it reported enough to compute one.
    pub fn tokens_per_second(&self) -> Option<f64> {
        let tokens = self.generated_tokens?;
        let nanos = self.generation_duration_ns?;
        (nanos > 0).then(|| tokens as f64 / (nanos as f64 / 1e9))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelChunk {
    pub content: String,
    /// Reasoning text when the deployment emits it on a separate channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Present only on the terminal chunk, and only where the backend reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<GenerationMetrics>,
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
        // A point that did not meet the profile's own admission criteria is not
        // a stable point. Without this, a tier where every sample failed still
        // authorises execution at that context.
        if let Some(point) = self
            .stable_points
            .iter()
            .find(|point| !self.thresholds.admits(point))
        {
            return Err(DomainError::Invalid {
                field: "stable_points",
                reason: format!(
                    "point at {} tokens does not meet the profile thresholds",
                    point.context_tokens
                ),
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
                    // The covering point must itself be admissible: capacity
                    // comes from a measurement that succeeded, not merely from
                    // one that was attempted at that size.
                    && profile.stable_points.iter().any(|p| {
                        p.context_tokens >= self.context_tokens
                            && profile.thresholds.admits(p)
                    }) =>
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
            thresholds: CalibrationThresholds::default(),
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
