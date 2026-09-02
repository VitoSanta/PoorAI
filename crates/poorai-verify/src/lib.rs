//! Deterministic verification baselines and bounded recovery taxonomy.
use poorai_domain::{hash_bytes, now};
use poorai_tools::{ToolError, ToolPolicy, ToolResult, run_command};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationBaseline {
    pub id: poorai_domain::Id,
    pub captured_at: chrono::DateTime<chrono::Utc>,
    pub checks: Vec<CheckRecord>,
    pub environment_hash: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckRecord {
    pub command: String,
    pub result: ToolResult,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineComparison {
    pub previous_baseline_id: poorai_domain::Id,
    pub current_baseline_id: poorai_domain::Id,
    pub new_failures: Vec<String>,
    pub regression_free: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FailureClass {
    Compilation,
    Assertion,
    Environment,
    Provider,
    Policy,
    NonDeterminism,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RecoveryDecision {
    EditAndRetry { remaining_edit_verify_cycles: u8 },
    RetryContextTier { remaining_context_retries: u8 },
    Stop { reason: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryBudget {
    pub max_edit_verify_cycles: u8,
    pub max_context_retries: u8,
}
impl Default for RecoveryBudget {
    fn default() -> Self {
        Self {
            max_edit_verify_cycles: 3,
            max_context_retries: 1,
        }
    }
}

/// Selects only deterministic, locally available checks from repository manifests.
pub fn discover_checks(
    root: &std::path::Path,
    scope: &str,
) -> Result<Vec<(String, Vec<String>)>, String> {
    if !matches!(scope, "targeted" | "full") {
        return Err("scope must be targeted or full".into());
    }
    if root.join("Cargo.toml").is_file() {
        let mut args = vec!["test".into(), "--workspace".into()];
        if scope == "targeted" {
            args.push("--lib".into());
        }
        return Ok(vec![("cargo".into(), args)]);
    }
    if let Some(manifest) = std::fs::read_to_string(root.join("package.json")).ok()
        && serde_json::from_str::<serde_json::Value>(&manifest)
            .ok()
            .and_then(|m| m.get("scripts")?.get("test").cloned())
            .is_some()
    {
        return Ok(vec![("npm".into(), vec!["test".into(), "--silent".into()])]);
    }
    // A repository with no verifier we recognise is not an error: it has no
    // deterministic checks, which the caller records rather than refuses. A
    // run against it simply cannot claim verification, and completion is
    // judged on nothing rather than on something invented.
    Ok(Vec::new())
}
pub async fn baseline(
    policy: &ToolPolicy,
    checks: &[(String, Vec<String>)],
) -> Result<VerificationBaseline, ToolError> {
    let mut records = Vec::new();
    for (cmd, args) in checks {
        records.push(CheckRecord {
            command: format!("{} {}", cmd, args.join(" ")),
            result: run_command(policy, cmd, args).await?,
        });
    }
    let stable_record = records
        .iter()
        .map(|record| {
            (
                &record.command,
                record.result.exit_code,
                &record.result.artifact_hash,
            )
        })
        .collect::<Vec<_>>();
    let h = hash_bytes(serde_json::to_vec(&stable_record).unwrap_or_default());
    Ok(VerificationBaseline {
        id: poorai_domain::new_id(),
        captured_at: now(),
        checks: records,
        environment_hash: h,
    })
}
pub fn compare(
    previous: &VerificationBaseline,
    current: &VerificationBaseline,
) -> BaselineComparison {
    let new_failures = current
        .checks
        .iter()
        .filter(|current_check| {
            current_check.result.exit_code != Some(0)
                && previous
                    .checks
                    .iter()
                    .find(|prior| prior.command == current_check.command)
                    .is_some_and(|prior| prior.result.exit_code == Some(0))
        })
        .map(|check| check.command.clone())
        .collect::<Vec<_>>();
    BaselineComparison {
        previous_baseline_id: previous.id,
        current_baseline_id: current.id,
        regression_free: new_failures.is_empty(),
        new_failures,
    }
}
/// Re-runs a failing check to tell a real failure from a flake.
///
/// Without this, `NonDeterminism` is unreachable: a flaky test classifies as
/// `Assertion`, which authorises an edit-and-retry cycle, so the agent edits
/// working code to chase a failure that was never in the code. A check whose
/// outcome changes on identical inputs is non-deterministic, and the recovery
/// taxonomy stops rather than edits.
pub async fn classify_with_reproduction(
    policy: &ToolPolicy,
    command: &str,
    args: &[String],
    first: &ToolResult,
) -> Result<FailureClass, ToolError> {
    if first.exit_code == Some(0) {
        return Ok(classify(first));
    }
    let second = run_command(policy, command, args).await?;
    if second.exit_code != first.exit_code {
        return Ok(FailureClass::NonDeterminism);
    }
    Ok(classify(first))
}

pub fn classify(result: &ToolResult) -> FailureClass {
    if result.stderr.contains("error[") || result.stderr.contains("error:") {
        FailureClass::Compilation
    } else if result.stderr.contains("assert") || result.stdout.contains("FAILED") {
        FailureClass::Assertion
    } else {
        FailureClass::Environment
    }
}
/// Decides recovery without granting any tool authority. Infrastructure failures never authorize edits.
pub fn recovery_decision(
    class: FailureClass,
    edit_attempts: u8,
    context_attempts: u8,
    budget: &RecoveryBudget,
) -> RecoveryDecision {
    match class {
        FailureClass::Compilation | FailureClass::Assertion
            if edit_attempts < budget.max_edit_verify_cycles =>
        {
            RecoveryDecision::EditAndRetry {
                remaining_edit_verify_cycles: budget.max_edit_verify_cycles - edit_attempts,
            }
        }
        FailureClass::Provider if context_attempts < budget.max_context_retries => {
            RecoveryDecision::RetryContextTier {
                remaining_context_retries: budget.max_context_retries - context_attempts,
            }
        }
        FailureClass::Compilation | FailureClass::Assertion => RecoveryDecision::Stop {
            reason: "edit-verify budget exhausted".into(),
        },
        FailureClass::Environment => RecoveryDecision::Stop {
            reason: "environment failure: classify and repair infrastructure before editing".into(),
        },
        FailureClass::Policy => RecoveryDecision::Stop {
            reason: "policy denial requires explicit authorization".into(),
        },
        FailureClass::NonDeterminism => RecoveryDecision::Stop {
            reason: "non-deterministic verification requires a reproducible failure".into(),
        },
        FailureClass::Provider => RecoveryDecision::Stop {
            reason: "context retry budget exhausted".into(),
        },
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn environment_failure_never_authorizes_edit() {
        assert!(matches!(
            recovery_decision(FailureClass::Environment, 0, 0, &RecoveryBudget::default()),
            RecoveryDecision::Stop { .. }
        ));
    }
    #[test]
    fn bounded_code_recovery_stops_after_budget() {
        let budget = RecoveryBudget {
            max_edit_verify_cycles: 1,
            max_context_retries: 1,
        };
        assert!(matches!(
            recovery_decision(FailureClass::Assertion, 0, 0, &budget),
            RecoveryDecision::EditAndRetry { .. }
        ));
        assert!(matches!(
            recovery_decision(FailureClass::Assertion, 1, 0, &budget),
            RecoveryDecision::Stop { .. }
        ));
    }
}
