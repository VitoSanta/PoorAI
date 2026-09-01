//! Durable task-state transitions and evidence-bounded profile selection.
use futures_util::StreamExt;
use poorai_domain::{
    CalibrationProfile, DeploymentDescriptor, EvidenceLabel, ExecutionProfile, HardwareProfile,
    Observation, RuntimeSnapshot, Validate, new_id, now,
};
use poorai_provider::ModelProvider;
use poorai_store::Store;
use poorai_tools::{ActionProposal, ToolPolicy};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Instant;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskState {
    Discover,
    Profile,
    Index,
    Plan,
    Act,
    Verify,
    Recover,
    Complete,
    Failed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCheckpoint {
    pub id: poorai_domain::Id,
    pub state: TaskState,
    pub at: chrono::DateTime<chrono::Utc>,
    pub detail: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DryRunPlan {
    pub run_id: poorai_domain::Id,
    pub workspace_root: String,
    pub task: String,
    pub model: Option<String>,
    pub checkpoints: Vec<TaskCheckpoint>,
    pub index_hash: String,
    pub indexed_files: usize,
    pub intended_tools: Vec<String>,
    pub expected_checks: Vec<String>,
    pub files_to_inspect: Vec<String>,
    pub stop_condition: String,
}

/// Builds a durable pre-action plan without authorizing model actions or edits.
pub fn prepare_dry_run(
    task: String,
    model: Option<String>,
    index: &poorai_repo::RepositoryIndex,
) -> Result<DryRunPlan, String> {
    if task.trim().is_empty() {
        return Err("task must not be empty".into());
    }
    let discover = transition(
        TaskState::Discover,
        TaskState::Profile,
        "workspace root resolved",
    )?;
    let profile = transition(
        TaskState::Profile,
        TaskState::Index,
        "dry run: execution profile intentionally absent",
    )?;
    let indexed = transition(
        TaskState::Index,
        TaskState::Plan,
        "repository inventory captured",
    )?;
    Ok(DryRunPlan {
        run_id: new_id(),
        workspace_root: index.root.clone(),
        task,
        model,
        checkpoints: vec![discover, profile, indexed],
        index_hash: index.inventory_hash.clone(),
        indexed_files: index.files.len(),
        intended_tools: vec![
            "ListTree".into(),
            "Search".into(),
            "ReadFile".into(),
            "GitDiff".into(),
            "RunCommand".into(),
        ],
        expected_checks: if index.files.iter().any(|file| file.path == "Cargo.toml") {
            vec!["cargo test --workspace --lib".into()]
        } else {
            vec![]
        },
        files_to_inspect: index
            .files
            .iter()
            .filter(|file| file.path.ends_with(".rs") || file.path == "Cargo.toml")
            .take(20)
            .map(|file| file.path.clone())
            .collect(),
        stop_condition: "dry run stops before model invocation, edits, or verification".into(),
    })
}
pub fn transition(
    from: TaskState,
    to: TaskState,
    detail: impl Into<String>,
) -> Result<TaskCheckpoint, String> {
    let legal = matches!(
        (from.clone(), to.clone()),
        (TaskState::Discover, TaskState::Profile)
            | (TaskState::Profile, TaskState::Index)
            | (TaskState::Index, TaskState::Plan)
            | (TaskState::Plan, TaskState::Act)
            | (TaskState::Act, TaskState::Verify)
            | (TaskState::Verify, TaskState::Complete)
            | (TaskState::Verify, TaskState::Recover)
            | (TaskState::Recover, TaskState::Act)
            | (_, TaskState::Failed)
    );
    if !legal {
        return Err(format!("illegal transition {from:?} -> {to:?}"));
    }
    Ok(TaskCheckpoint {
        id: new_id(),
        state: to,
        at: now(),
        detail: detail.into(),
    })
}
pub trait HardwareProbe: Send + Sync {
    fn probe(&self, workspace_root: &Path) -> Result<HardwareProfile, String>;
}
pub fn snapshot(
    profile: &HardwareProfile,
    deployment: &DeploymentDescriptor,
    available_memory_bytes: Option<u64>,
    pressure: Observation,
    backend_state: serde_json::Value,
) -> RuntimeSnapshot {
    RuntimeSnapshot {
        schema_version: 1,
        id: new_id(),
        hardware_id: profile.id,
        deployment_id: deployment.id,
        timestamp: now(),
        available_memory_bytes,
        pressure,
        loaded_models: vec![],
        backend_state,
    }
}
pub fn select_profile(
    strategy_id: poorai_domain::Id,
    calibration: Option<&CalibrationProfile>,
    compatibility_key: &str,
) -> Result<ExecutionProfile, String> {
    if let Some(c) = calibration {
        c.validate().map_err(|e| e.to_string())?;
        let point = c
            .stable_points
            .iter()
            .filter(|p| !p.memory_pressure_observed)
            .max_by_key(|p| p.context_tokens)
            .ok_or("no stable calibration point")?;
        return Ok(ExecutionProfile {
            schema_version: 1,
            id: new_id(),
            strategy_id,
            calibration_id: Some(c.id),
            context_tokens: point.context_tokens,
            reserve_tokens: (point.context_tokens / 8).max(1),
            concurrency: 1,
            budgets: serde_json::json!({"max_actions":8,"edit_verify_cycles":3,"context_retries":1}),
            rationale: "highest compatible measured stable point".into(),
            evidence: EvidenceLabel::Measured,
            compatibility_key: compatibility_key.into(),
        });
    }
    Err("no compatible calibration evidence; create an explicitly labelled bootstrap profile or calibrate".into())
}

/// Refuses to construct an execution profile when calibration provenance no longer matches.
pub fn select_compatible_profile(
    strategy_id: poorai_domain::Id,
    calibration: &CalibrationProfile,
    model_digest: &str,
    deployment: &DeploymentDescriptor,
    hardware: &HardwareProfile,
    harness_rev: &str,
) -> Result<ExecutionProfile, String> {
    if calibration.model_digest != model_digest {
        return Err("calibration invalid: model digest changed".into());
    }
    if calibration.deployment_fingerprint != deployment.fingerprint() {
        return Err("calibration invalid: deployment fingerprint changed".into());
    }
    if calibration.compatibility_key != hardware.compatibility_key {
        return Err("calibration invalid: hardware compatibility key changed".into());
    }
    if calibration.harness_rev != harness_rev {
        return Err("calibration invalid: harness revision changed".into());
    }
    select_profile(strategy_id, Some(calibration), &hardware.compatibility_key)
}

/// Executes one validated, typed action and durably records its outcome before another action may run.
/// Executes one typed action under policy and audits the attempt.
///
/// Every attempt is recorded, allowed or denied. A policy denial is the security
/// boundary doing its job, and it is the event most worth having: an audit log
/// that holds only successes cannot show that anything was ever refused.
pub async fn execute_action(
    store: &Store,
    run_id: poorai_domain::Id,
    policy: &ToolPolicy,
    action: ActionProposal,
) -> Result<serde_json::Value, String> {
    let result = attempt_action(policy, &action).await;
    let payload = match &result {
        Ok(outcome) => serde_json::json!({
            "action": action,
            "status": "allowed",
            "outcome": outcome,
        }),
        Err(denial) => serde_json::json!({
            "action": action,
            "status": "denied",
            "denial": denial,
        }),
    };
    // The audit is written before the denial propagates, so a refused action
    // cannot leave the run without a record of what was asked.
    store
        .append(Some(run_id), "tool.action", payload)
        .map_err(|e| e.to_string())?;
    result
}

/// Runs one action under policy, without auditing. Callers go through
/// `execute_action` so the attempt is recorded either way.
async fn attempt_action(
    policy: &ToolPolicy,
    action: &ActionProposal,
) -> Result<serde_json::Value, String> {
    action.validate().map_err(|e| e.to_string())?;
    match action {
        ActionProposal::Complete { rationale } => {
            Ok(serde_json::json!({"complete":true,"rationale":rationale}))
        }
        ActionProposal::ReadFile { path } => serde_json::to_value(
            poorai_tools::read_file(policy, std::path::Path::new(path))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string()),
        ActionProposal::Search { query, max_matches } => serde_json::to_value(
            poorai_tools::search(policy, query, *max_matches).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string()),
        ActionProposal::ListTree { max_entries } => serde_json::to_value(
            poorai_tools::list_tree(policy, *max_entries).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string()),
        ActionProposal::ApplyReplace {
            path,
            expected_hash,
            replacement,
        } => serde_json::to_value(
            poorai_tools::apply_replace(
                policy,
                std::path::Path::new(path),
                expected_hash,
                replacement,
            )
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string()),
        ActionProposal::RunCommand { executable, args } => serde_json::to_value(
            poorai_tools::run_command(policy, executable, args)
                .await
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string()),
    }
}

pub fn checkpoint_recovery(
    store: &Store,
    run_id: poorai_domain::Id,
    class: poorai_verify::FailureClass,
    edit_attempts: u8,
    context_attempts: u8,
) -> Result<poorai_verify::RecoveryDecision, String> {
    let decision = poorai_verify::recovery_decision(
        class,
        edit_attempts,
        context_attempts,
        &poorai_verify::RecoveryBudget::default(),
    );
    store.append(Some(run_id),"task.recovery",serde_json::json!({"decision":decision,"edit_attempts":edit_attempts,"context_attempts":context_attempts})).map_err(|e|e.to_string())?;
    Ok(decision)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRunResult {
    pub run_id: poorai_domain::Id,
    pub verified: bool,
    pub action_outcome: serde_json::Value,
}

pub async fn run_single_action<P: ModelProvider>(
    store: &Store,
    provider: &P,
    request: poorai_domain::ModelRequest,
    policy: &ToolPolicy,
    checks: &[(String, Vec<String>)],
) -> Result<TaskRunResult, String> {
    let run_id = new_id();
    let before = poorai_verify::baseline(policy, checks)
        .await
        .map_err(|e| e.to_string())?;
    store
        .append(
            Some(run_id),
            "verification.baseline",
            serde_json::to_value(&before).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    let mut stream = provider.chat(request).await.map_err(|e| e.to_string())?;
    let chunk = stream
        .next()
        .await
        .ok_or("provider returned empty stream")?
        .map_err(|e| e.to_string())?;
    let action = parse_action_proposal(&chunk.content)?;
    let outcome = execute_action(store, run_id, policy, action).await?;
    let after = poorai_verify::baseline(policy, checks)
        .await
        .map_err(|e| e.to_string())?;
    let comparison = poorai_verify::compare(&before, &after);
    let verified = after
        .checks
        .iter()
        .all(|check| check.result.exit_code == Some(0))
        && comparison.regression_free;
    store
        .append(
            Some(run_id),
            "verification.result",
            serde_json::json!({"after":after,"comparison":comparison,"verified":verified}),
        )
        .map_err(|e| e.to_string())?;
    Ok(TaskRunResult {
        run_id,
        verified,
        action_outcome: outcome,
    })
}

/// Executes a bounded reasoning/action loop. Completion is accepted only after deterministic checks pass.
pub async fn run_action_loop<P: ModelProvider>(
    store: &Store,
    provider: &P,
    mut request: poorai_domain::ModelRequest,
    policy: &ToolPolicy,
    checks: &[(String, Vec<String>)],
    max_actions: u8,
) -> Result<TaskRunResult, String> {
    let run_id = new_id();
    let before = poorai_verify::baseline(policy, checks)
        .await
        .map_err(|e| e.to_string())?;
    store
        .append(
            Some(run_id),
            "verification.baseline",
            serde_json::to_value(&before).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    for step in 0..max_actions {
        let mut stream = provider
            .chat(request.clone())
            .await
            .map_err(|e| e.to_string())?;
        let chunk = stream
            .next()
            .await
            .ok_or("provider returned empty stream")?
            .map_err(|e| e.to_string())?;
        let action = parse_action_proposal(&chunk.content)?;
        if matches!(action, ActionProposal::Complete { .. }) {
            let after = poorai_verify::baseline(policy, checks)
                .await
                .map_err(|e| e.to_string())?;
            let comparison = poorai_verify::compare(&before, &after);
            let verified = after
                .checks
                .iter()
                .all(|check| check.result.exit_code == Some(0))
                && comparison.regression_free;
            store
                .append(
                    Some(run_id),
                    "verification.result",
                    serde_json::json!({"after":after,"comparison":comparison,"verified":verified}),
                )
                .map_err(|e| e.to_string())?;
            if verified {
                store
                    .append(
                        Some(run_id),
                        "task.complete",
                        serde_json::json!({"step":step}),
                    )
                    .map_err(|e| e.to_string())?;
                return Ok(TaskRunResult {
                    run_id,
                    verified,
                    action_outcome: serde_json::json!({"complete":true,"step":step}),
                });
            }
            let decision = checkpoint_recovery(
                store,
                run_id,
                poorai_verify::FailureClass::Assertion,
                step,
                0,
            )?;
            if matches!(decision, poorai_verify::RecoveryDecision::Stop { .. }) {
                store
                    .append(
                        Some(run_id),
                        "task.failed",
                        serde_json::json!({"reason":"recovery budget exhausted"}),
                    )
                    .map_err(|e| e.to_string())?;
                return Err("verification failed and recovery budget exhausted".into());
            }
            request.messages.push(poorai_domain::ChatMessage {
                role: "tool".into(),
                content: serde_json::json!({"verification_failed":true,"recovery":decision})
                    .to_string(),
            });
            continue;
        }
        let outcome = execute_action(store, run_id, policy, action).await?;
        request.messages.push(poorai_domain::ChatMessage {
            role: "tool".into(),
            content: serde_json::to_string(&outcome).map_err(|e| e.to_string())?,
        });
    }
    store
        .append(
            Some(run_id),
            "task.failed",
            serde_json::json!({"reason":"action budget exhausted"}),
        )
        .map_err(|e| e.to_string())?;
    Err("action budget exhausted before verified completion".into())
}

/// Parses exactly one JSON action proposal; prose and fenced output are rejected.
pub fn parse_action_proposal(model_output: &str) -> Result<ActionProposal, String> {
    let action: ActionProposal = serde_json::from_str(model_output.trim())
        .map_err(|_| "model output must be one valid typed-action JSON object".to_string())?;
    action.validate().map_err(|e| e.to_string())?;
    Ok(action)
}

/// Runs three fixed-prompt samples for every requested context tier.
pub async fn calibrate<P: ModelProvider>(
    provider: &P,
    deployment: &DeploymentDescriptor,
    hardware: &HardwareProfile,
    model_digest: String,
    ladder: &[u32],
    harness_rev: &str,
) -> Result<CalibrationProfile, String> {
    if ladder.is_empty() || ladder.contains(&0) {
        return Err("context ladder must contain positive values".into());
    }
    let mut points = Vec::new();
    let mut artifacts = Vec::new();
    for context_tokens in ladder {
        let mut latencies = Vec::new();
        let mut successes = 0usize;
        for _ in 0..3 {
            let request = poorai_domain::ModelRequest {
                deployment: deployment.clone(),
                context_tokens: *context_tokens,
                tools: None,
                messages: vec![poorai_domain::ChatMessage {
                    role: "user".into(),
                    content: "Reply with the single token: OK".into(),
                }],
            };
            let started = Instant::now();
            let outcome = match provider.chat(request).await {
                Ok(mut stream) => match stream.next().await {
                    Some(Ok(chunk)) => {
                        successes += 1;
                        serde_json::json!({"ok":true,"done":chunk.done,"bytes":chunk.content.len()})
                    }
                    Some(Err(error)) => serde_json::json!({"ok":false,"error":error.to_string()}),
                    None => serde_json::json!({"ok":false,"error":"empty stream"}),
                },
                Err(error) => serde_json::json!({"ok":false,"error":error.to_string()}),
            };
            latencies.push(started.elapsed().as_secs_f64() * 1000.0);
            artifacts.push(poorai_domain::hash_bytes(
                serde_json::to_vec(&outcome).unwrap_or_default(),
            ));
        }
        latencies.sort_by(f64::total_cmp);
        let median = latencies[latencies.len() / 2];
        let mean = latencies.iter().sum::<f64>() / latencies.len() as f64;
        let variance =
            latencies.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / latencies.len() as f64;
        points.push(poorai_domain::StablePoint {
            context_tokens: *context_tokens,
            samples: 3,
            success_rate: successes as f64 / 3.0,
            median_first_token_ms: median,
            generation_tokens_per_second: 0.0,
            variance,
            memory_pressure_observed: false,
        });
    }
    let profile = CalibrationProfile {
        schema_version: 1,
        id: new_id(),
        compatibility_key: hardware.compatibility_key.clone(),
        model_digest,
        deployment_fingerprint: deployment.fingerprint(),
        harness_rev: harness_rev.into(),
        stable_points: points,
        raw_artifact_hashes: artifacts,
        created_at: now(),
    };
    profile.validate().map_err(|e| e.to_string())?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures_util::stream;
    use poorai_domain::{BackendState, ModelChunk, ModelInspection, ModelRequest, Provenance};
    use poorai_provider::{ModelStream, ProviderError};
    use poorai_store::Store;
    use poorai_tools::ToolPolicy;
    use std::time::Duration;

    struct FakeProvider;
    #[async_trait]
    impl ModelProvider for FakeProvider {
        async fn inspect(
            &self,
            _: &DeploymentDescriptor,
        ) -> Result<ModelInspection, ProviderError> {
            unreachable!()
        }
        async fn runtime_state(&self) -> Result<BackendState, ProviderError> {
            unreachable!()
        }
        async fn chat(&self, _: ModelRequest) -> Result<ModelStream, ProviderError> {
            Ok(Box::pin(stream::iter([Ok(ModelChunk {
                content: "OK".into(),
                thinking: None,
                tool_calls: Vec::new(),
                done: true,
            })])))
        }
    }
    struct ActionProvider;
    #[async_trait]
    impl ModelProvider for ActionProvider {
        async fn inspect(
            &self,
            _: &DeploymentDescriptor,
        ) -> Result<ModelInspection, ProviderError> {
            unreachable!()
        }
        async fn runtime_state(&self) -> Result<BackendState, ProviderError> {
            unreachable!()
        }
        async fn chat(&self, _: ModelRequest) -> Result<ModelStream, ProviderError> {
            Ok(Box::pin(stream::iter([Ok(ModelChunk {
                content: r#"{"capability":"read_file","path":"fixture.txt"}"#.into(),
                thinking: None,
                tool_calls: Vec::new(),
                done: true,
            })])))
        }
    }
    struct SequenceProvider(std::sync::Mutex<std::collections::VecDeque<String>>);
    #[async_trait]
    impl ModelProvider for SequenceProvider {
        async fn inspect(
            &self,
            _: &DeploymentDescriptor,
        ) -> Result<ModelInspection, ProviderError> {
            unreachable!()
        }
        async fn runtime_state(&self) -> Result<BackendState, ProviderError> {
            unreachable!()
        }
        async fn chat(&self, _: ModelRequest) -> Result<ModelStream, ProviderError> {
            let item = self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(ProviderError::Protocol {
                    safe_context: "sequence exhausted".into(),
                })?;
            Ok(Box::pin(stream::iter([Ok(ModelChunk {
                content: item,
                thinking: None,
                tool_calls: Vec::new(),
                done: true,
            })])))
        }
    }
    fn hardware() -> HardwareProfile {
        HardwareProfile {
            schema_version: 1,
            id: new_id(),
            compatibility_key: "compat".into(),
            os: "test".into(),
            architecture: "test".into(),
            cpu: "test".into(),
            accelerators: vec![],
            total_memory_bytes: None,
            storage_free_bytes: None,
            unavailable_fields: vec![],
            probe_version: "test".into(),
            provenance: Provenance {
                source: "test".into(),
                observed_at: now(),
                content_hash: "x".into(),
            },
        }
    }
    fn deployment() -> DeploymentDescriptor {
        DeploymentDescriptor {
            schema_version: 1,
            id: new_id(),
            provider: "fake".into(),
            endpoint: "http://localhost/".into(),
            model_ref: "fake".into(),
            backend_options: Default::default(),
            auth_ref: None,
        }
    }
    #[tokio::test]
    async fn calibration_has_three_samples_per_tier() {
        let profile = calibrate(
            &FakeProvider,
            &deployment(),
            &hardware(),
            "digest".into(),
            &[32, 64],
            "harness",
        )
        .await
        .unwrap();
        assert_eq!(profile.stable_points.len(), 2);
        assert!(
            profile
                .stable_points
                .iter()
                .all(|p| p.samples == 3 && p.success_rate == 1.0)
        );
        assert_eq!(profile.raw_artifact_hashes.len(), 6);
    }
    #[test]
    fn action_parser_rejects_unstructured_model_output() {
        assert!(parse_action_proposal("I would read a file").is_err());
        assert!(matches!(
            parse_action_proposal(r#"{"capability":"read_file","path":"src/lib.rs"}"#),
            Ok(ActionProposal::ReadFile { .. })
        ));
    }
    #[tokio::test]
    async fn locked_smoke_action_is_audited() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("fixture.txt"), "safe").unwrap();
        let policy = ToolPolicy {
            root: root.path().to_path_buf(),
            allow_commands: vec![],
            output_limit: 128,
            timeout: Duration::from_secs(1),
            network_enabled: false,
        };
        let store = Store::open(":memory:").unwrap();
        let run_id = new_id();
        let action =
            parse_action_proposal(r#"{"capability":"read_file","path":"fixture.txt"}"#).unwrap();
        let outcome = execute_action(&store, run_id, &policy, action)
            .await
            .unwrap();
        assert_eq!(outcome["content"], "safe");
        assert_eq!(store.events_for_run(run_id).unwrap().len(), 1);
    }
    #[tokio::test]
    async fn locked_smoke_controller_verifies_one_action() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("fixture.txt"), "safe").unwrap();
        let policy = ToolPolicy {
            root: root.path().to_path_buf(),
            allow_commands: vec!["cargo".into()],
            output_limit: 1024,
            timeout: Duration::from_secs(10),
            network_enabled: false,
        };
        let store = Store::open(":memory:").unwrap();
        let request = ModelRequest {
            deployment: deployment(),
            context_tokens: 32,
            tools: None,
            messages: vec![],
        };
        let result = run_single_action(&store, &ActionProvider, request, &policy, &[])
            .await
            .unwrap();
        assert!(result.verified);
        assert_eq!(store.events_for_run(result.run_id).unwrap().len(), 3);
    }
    #[tokio::test]
    async fn locked_smoke_recovers_then_verifies() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("marker"), "pass").unwrap();
        std::fs::write(
            root.path().join("check.sh"),
            "test \"$(cat marker)\" = pass",
        )
        .unwrap();
        let policy = ToolPolicy {
            root: root.path().to_path_buf(),
            allow_commands: vec!["sh".into()],
            output_limit: 1024,
            timeout: Duration::from_secs(10),
            network_enabled: false,
        };
        let pass = poorai_domain::hash_bytes("pass");
        let fail = poorai_domain::hash_bytes("fail");
        let provider = SequenceProvider(std::sync::Mutex::new(std::collections::VecDeque::from([
            format!(
                r#"{{"capability":"apply_replace","path":"marker","expected_hash":"{pass}","replacement":"fail"}}"#
            ),
            r#"{"capability":"complete","rationale":"done"}"#.into(),
            format!(
                r#"{{"capability":"apply_replace","path":"marker","expected_hash":"{fail}","replacement":"pass"}}"#
            ),
            r#"{"capability":"complete","rationale":"fixed"}"#.into(),
        ])));
        let store = Store::open(":memory:").unwrap();
        let request = ModelRequest {
            deployment: deployment(),
            context_tokens: 32,
            tools: None,
            messages: vec![],
        };
        let checks = vec![("sh".into(), vec!["check.sh".into()])];
        let result = run_action_loop(&store, &provider, request, &policy, &checks, 4)
            .await
            .unwrap();
        assert!(result.verified);
        assert!(
            store
                .events_for_run(result.run_id)
                .unwrap()
                .iter()
                .any(|event| event.event_type == "task.recovery")
        );
    }
}
