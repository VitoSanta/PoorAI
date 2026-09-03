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

/// Where a parameter's value came from.
///
/// Recorded per parameter because a run that reports a value without its
/// origin cannot be compared with another: a temperature the vendor
/// recommends, one a package happened to ship, and one nobody chose are three
/// different claims that look identical in a report.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParameterSource {
    /// The vendor's published recommendation for this model.
    OfficialModelCard,
    /// Declared in the packaged Modelfile the backend serves.
    OllamaModel,
    /// Chosen by poorAI deliberately, against a stated reason.
    PoorAiOverride,
    /// Derived from measurement on this machine.
    HardwareCalibration,
    /// Nothing set it. The backend decides, and we do not know what it decides.
    BackendDefault,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedParameter {
    pub value: serde_json::Value,
    pub source: ParameterSource,
}

/// How a deployment's reasoning depth is controlled.
///
/// Three different mechanisms, named separately because they are not
/// interchangeable: one is a backend option, one is a line the system prompt
/// must carry, and one is a request field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReasoningControl {
    /// A backend option, such as `reasoning_effort`.
    BackendOption { name: String, value: String },
    /// A directive the system prompt must contain, such as a reasoning
    /// strength line.
    PromptDirective { text: String },
    /// The backend's own thinking toggle.
    Think { enabled: bool },
}

/// Context sizes for one deployment tag.
///
/// Per tag rather than per family: the same model published under different
/// tags can declare different limits, and a family-level number would be wrong
/// for at least one of them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextPolicy {
    /// Below this an agent is too constrained to work; a run refuses rather
    /// than proceeding quietly.
    pub minimum: u32,
    /// Normal allocation.
    pub default: u32,
    /// The tag's declared limit.
    pub maximum: u32,
}

impl ContextPolicy {
    /// A policy whose sizes contradict each other would clamp to something
    /// nobody chose.
    pub fn is_coherent(&self) -> bool {
        self.minimum <= self.default && self.default <= self.maximum
    }
}

/// Everything about how one deployment should be driven.
///
/// Separate from `ModelDefinition`, which is facts the backend reported, and
/// from `ModelStrategy`, which is how the agent behaves. This is how the
/// request is built.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelProfile {
    pub schema_version: u32,
    /// Matched against the deployment's model reference, exactly.
    pub model_selector: String,
    pub context: ContextPolicy,
    /// Sampling options sent to the backend, each with its origin.
    pub sampling: BTreeMap<String, ResolvedParameter>,
    #[serde(default)]
    pub reasoning: Option<ReasoningControl>,
    /// Where the context default came from. A size measured on this machine is
    /// a different claim from one copied out of a specification.
    #[serde(default = "declared_source")]
    pub context_source: ParameterSource,
    /// Where these values came from, in words a reader can check.
    pub provenance: String,
}

impl ModelProfile {
    pub fn select<'a>(profiles: &'a [ModelProfile], model_ref: &str) -> Option<&'a ModelProfile> {
        profiles
            .iter()
            .find(|profile| profile.model_selector == model_ref)
    }

    /// The context to allocate, bounded by the tag's own limits.
    ///
    /// A request for more than the tag declares is clamped rather than sent:
    /// the backend would either refuse it or silently ignore it, and both make
    /// the recorded number a fiction.
    pub fn context_for(&self, requested: Option<u32>) -> u32 {
        requested
            .unwrap_or(self.context.default)
            .clamp(self.context.minimum, self.context.maximum)
    }

    /// Sampling options as the backend expects them.
    pub fn sampling_options(&self) -> BTreeMap<String, serde_json::Value> {
        self.sampling
            .iter()
            .map(|(name, resolved)| (name.clone(), resolved.value.clone()))
            .collect()
    }
}

fn declared_source() -> ParameterSource {
    ParameterSource::OfficialModelCard
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
    /// Ask for a plan before acting. Costs a turn, so it is opt-in and must be
    /// measured against the default rather than assumed to help.
    #[serde(default)]
    pub plan_first: bool,
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

/// Bounded controller budgets carried by an execution profile.
///
/// This typed view keeps persisted JSON forwards-compatible while preventing
/// callers from silently substituting hard-coded recovery limits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionBudgets {
    pub max_actions: u8,
    pub edit_verify_cycles: u8,
    pub context_retries: u8,
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
    /// Sampling options sent verbatim to the backend.
    ///
    /// A map rather than a field per parameter, because the set differs by
    /// vendor: one model recommends top_k and min_p, another recommends
    /// nothing, and inventing a value for a model whose vendor did not
    /// recommend one is a configuration nobody chose.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sampling: BTreeMap<String, serde_json::Value>,
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
    pub fn execution_budgets(&self) -> Result<ExecutionBudgets, DomainError> {
        let budgets: ExecutionBudgets =
            serde_json::from_value(self.budgets.clone()).map_err(|e| DomainError::Invalid {
                field: "budgets",
                reason: format!(
                    "expected max_actions, edit_verify_cycles, and context_retries: {e}"
                ),
            })?;
        if budgets.max_actions == 0 || budgets.edit_verify_cycles == 0 {
            return Err(DomainError::Invalid {
                field: "budgets",
                reason: "max_actions and edit_verify_cycles must be greater than zero".into(),
            });
        }
        Ok(budgets)
    }

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
        self.execution_budgets()?;
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
