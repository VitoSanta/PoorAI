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

/// A build system poorAI can recognise, as data rather than as a branch.
///
/// Adding a language is adding a row. A registry keyed on marker files is the
/// only shape that satisfies "verification appropriate to the repository"
/// without the set of repositories being decided here.
pub struct BuildSystem {
    pub name: &'static str,
    /// A file whose presence identifies the project. Checked in order, so more
    /// specific markers precede more general ones.
    pub marker: &'static str,
    /// The toolchain binary. Also what the run must be allowed to execute:
    /// a project whose own tools are denied cannot be verified.
    pub executable: &'static str,
    /// Arguments for a narrow check, run after each edit.
    pub targeted: &'static [&'static str],
    /// Arguments for the full suite.
    pub full: &'static [&'static str],
}

/// Recognised build systems, most specific marker first.
///
/// A repository matching none of these is not silently unverifiable: it can
/// declare its own checks, and if it does neither the run records that it had
/// no verifier rather than proceeding as though it passed.
pub const BUILD_SYSTEMS: &[BuildSystem] = &[
    BuildSystem {
        name: "cargo",
        marker: "Cargo.toml",
        executable: "cargo",
        targeted: &["test", "--workspace", "--lib"],
        full: &["test", "--workspace"],
    },
    BuildSystem {
        name: "go",
        marker: "go.mod",
        executable: "go",
        targeted: &["test", "./..."],
        full: &["test", "./..."],
    },
    BuildSystem {
        name: "maven",
        marker: "pom.xml",
        executable: "mvn",
        targeted: &["-q", "test"],
        full: &["-q", "verify"],
    },
    BuildSystem {
        name: "gradle",
        marker: "build.gradle",
        executable: "gradle",
        targeted: &["test"],
        full: &["build"],
    },
    BuildSystem {
        name: "gradle-kotlin",
        marker: "build.gradle.kts",
        executable: "gradle",
        targeted: &["test"],
        full: &["build"],
    },
    BuildSystem {
        name: "dotnet",
        marker: "global.json",
        executable: "dotnet",
        targeted: &["test", "--nologo"],
        full: &["test", "--nologo"],
    },
    BuildSystem {
        name: "swift",
        marker: "Package.swift",
        executable: "swift",
        targeted: &["test"],
        full: &["test"],
    },
    BuildSystem {
        name: "flutter",
        marker: "pubspec.yaml",
        executable: "flutter",
        targeted: &["test"],
        full: &["test"],
    },
    BuildSystem {
        name: "elixir",
        marker: "mix.exs",
        executable: "mix",
        targeted: &["test"],
        full: &["test"],
    },
    BuildSystem {
        name: "poetry",
        marker: "poetry.lock",
        executable: "poetry",
        targeted: &["run", "pytest", "-q"],
        full: &["run", "pytest"],
    },
    BuildSystem {
        name: "python",
        marker: "pyproject.toml",
        executable: "pytest",
        targeted: &["-q"],
        full: &[],
    },
    BuildSystem {
        name: "python-legacy",
        marker: "setup.py",
        executable: "pytest",
        targeted: &["-q"],
        full: &[],
    },
    BuildSystem {
        name: "python-requirements",
        marker: "requirements.txt",
        executable: "pytest",
        targeted: &["-q"],
        full: &[],
    },
    BuildSystem {
        name: "ruby",
        marker: "Gemfile",
        executable: "bundle",
        targeted: &["exec", "rspec"],
        full: &["exec", "rspec"],
    },
    BuildSystem {
        name: "php",
        marker: "composer.json",
        executable: "composer",
        targeted: &["test"],
        full: &["test"],
    },
    BuildSystem {
        name: "make",
        marker: "Makefile",
        executable: "make",
        targeted: &["test"],
        full: &["test"],
    },
    BuildSystem {
        name: "cmake",
        marker: "CMakeLists.txt",
        executable: "ctest",
        targeted: &["--output-on-failure"],
        full: &["--output-on-failure"],
    },
];

/// Every toolchain binary a repository's own build systems need.
///
/// Derived from what the repository is, rather than being a fixed list: a
/// project whose tools are denied cannot be verified, and hard-coding the
/// permitted set decides in advance which languages the agent works in.
pub fn required_executables(root: &std::path::Path) -> Vec<String> {
    let mut executables: Vec<String> = BUILD_SYSTEMS
        .iter()
        .filter(|system| root.join(system.marker).is_file())
        .map(|system| system.executable.to_string())
        .collect();
    // A JavaScript project's runner is npm, and its runtime is node.
    if root.join("package.json").is_file() {
        executables.push("npm".into());
        executables.push("node".into());
    }
    if let Some(declared) = declared_checks(root) {
        executables.extend(declared.into_iter().map(|(executable, _)| executable));
    }
    executables.sort();
    executables.dedup();
    executables
}

/// Checks a repository declares for itself, at `.poorai/checks.json`.
///
/// The escape hatch that keeps the registry from being a closed world: a
/// project poorAI does not recognise says how it is verified, rather than
/// being worked on blind.
fn declared_checks(root: &std::path::Path) -> Option<Vec<(String, Vec<String>)>> {
    #[derive(serde::Deserialize)]
    struct Declared {
        checks: Vec<Check>,
    }
    #[derive(serde::Deserialize)]
    struct Check {
        executable: String,
        #[serde(default)]
        args: Vec<String>,
    }
    let bytes = std::fs::read(root.join(".poorai/checks.json")).ok()?;
    let declared: Declared = serde_json::from_slice(&bytes).ok()?;
    Some(
        declared
            .checks
            .into_iter()
            .map(|check| (check.executable, check.args))
            .collect(),
    )
}

/// Selects only deterministic, locally available checks from repository manifests.
pub fn discover_checks(
    root: &std::path::Path,
    scope: &str,
) -> Result<Vec<(String, Vec<String>)>, String> {
    if !matches!(scope, "targeted" | "full") {
        return Err("scope must be targeted or full".into());
    }
    // A repository's own declaration wins: it knows how it is verified and this
    // registry only guesses.
    if let Some(declared) = declared_checks(root) {
        return Ok(declared);
    }
    if let Some(manifest) = std::fs::read_to_string(root.join("package.json")).ok()
        && serde_json::from_str::<serde_json::Value>(&manifest)
            .ok()
            .and_then(|m| m.get("scripts")?.get("test").cloned())
            .is_some()
    {
        return Ok(vec![("npm".into(), vec!["test".into(), "--silent".into()])]);
    }
    for system in BUILD_SYSTEMS {
        if !root.join(system.marker).is_file() {
            continue;
        }
        // A build system with no distinct full command runs its targeted one:
        // some toolchains have a single test entry point and inventing a
        // second would be a command nobody chose.
        let args = match scope {
            "full" if !system.full.is_empty() => system.full,
            _ => system.targeted,
        };
        return Ok(vec![(
            system.executable.to_string(),
            args.iter().map(|a| a.to_string()).collect(),
        )]);
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
