//! Facts the loop has and the deployment does not.
//!
//! The dominant failure mode measured in this project is a repository
//! correctly fixed and the completion never declared — eleven of forty-eight
//! runs in one campaign, present in every deployment tested. A run is judged
//! against a budget the deployment cannot see, and after a long history it
//! cannot easily tell how long the checks have been passing either.

use async_trait::async_trait;
use poorai_domain::{
    BackendState, ChatMessage, DeploymentDescriptor, ModelChunk, ModelInspection, ModelRequest,
};
use poorai_provider::{ModelProvider, ModelStream, ProviderError};
use poorai_store::Store;
use poorai_tools::{SandboxPolicy, ToolPolicy};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Records every tool message it is given, then edits once and reads forever —
/// the measured shape of a run that fixes the repository and never says so.
struct WatchingProvider {
    turn: Arc<Mutex<usize>>,
    seen: Arc<Mutex<Vec<String>>>,
    hash: String,
}
#[async_trait]
impl ModelProvider for WatchingProvider {
    async fn inspect(&self, _: &DeploymentDescriptor) -> Result<ModelInspection, ProviderError> {
        unreachable!()
    }
    async fn runtime_state(&self) -> Result<BackendState, ProviderError> {
        unreachable!()
    }
    async fn chat(&self, request: ModelRequest) -> Result<ModelStream, ProviderError> {
        let mut seen = self.seen.lock().unwrap();
        seen.extend(
            request
                .messages
                .iter()
                .filter(|m| m.role == "tool")
                .map(|m| m.content.clone()),
        );
        drop(seen);
        let mut turn = self.turn.lock().unwrap();
        *turn += 1;
        let action = if *turn == 1 {
            serde_json::json!({
                "capability": "replace_text", "path": "code.rs",
                "expected_hash": self.hash, "find": "broken", "replace": "fixed"
            })
        } else {
            serde_json::json!({"capability": "read_file", "path": "code.rs"})
        };
        Ok(Box::pin(futures_util::stream::iter([Ok(ModelChunk {
            content: action.to_string(),
            done: true,
            ..Default::default()
        })])))
    }
}

fn run(max_actions: u8) -> Vec<String> {
    let root = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    std::fs::write(root.path().join("code.rs"), "broken").unwrap();
    let policy = ToolPolicy {
        root: root.path().to_path_buf(),
        allow_commands: vec!["true".into()],
        output_limit: 8192,
        timeout: Duration::from_secs(5),
        sandbox: SandboxPolicy::Disabled,
        approvals: Vec::new(),
    };
    let seen = Arc::new(Mutex::new(Vec::new()));
    let request = ModelRequest {
        deployment: DeploymentDescriptor {
            schema_version: 1,
            id: poorai_domain::new_id(),
            provider: "fake".into(),
            endpoint: "http://localhost/".into(),
            model_ref: "fake".into(),
            backend_options: Default::default(),
            auth_ref: None,
        },
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "fix it".into(),
        }],
        context_tokens: 8192,
        tools: None,
        seed: None,
        sampling: Default::default(),
    };
    let _ = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(poorai_orchestrator::run_action_loop(
            &Store::open(":memory:").unwrap(),
            &WatchingProvider {
                turn: Arc::new(Mutex::new(0)),
                seen: seen.clone(),
                hash: poorai_domain::hash_bytes("broken"),
            },
            poorai_domain::new_id(),
            request,
            &policy,
            &[("true".to_string(), vec![])],
            max_actions,
        ));
    seen.lock().unwrap().clone()
}

#[test]
fn the_deployment_is_told_how_many_actions_remain() {
    let seen = run(6);
    assert!(
        seen.iter().any(|m| m.contains("actions_remaining")),
        "a run is judged against a budget the deployment cannot see"
    );
    // Counting down, so the number means something across turns.
    let counts: Vec<i64> = seen
        .iter()
        .filter_map(|m| serde_json::from_str::<serde_json::Value>(m).ok())
        .filter_map(|v| v["status"]["actions_remaining"].as_i64())
        .collect();
    assert!(counts.len() >= 2);
    assert!(counts[0] > *counts.last().unwrap(), "{counts:?}");
}

/// After a long history a deployment cannot easily tell that the checks have
/// been passing and nothing has changed since. The loop can.
#[test]
fn the_deployment_is_told_the_checks_have_been_passing_and_nothing_changed() {
    let seen = run(6);
    let idle: Vec<i64> = seen
        .iter()
        .filter_map(|m| serde_json::from_str::<serde_json::Value>(m).ok())
        .filter_map(|v| v["status"]["actions_since_without_changing_a_file"].as_i64())
        .collect();
    assert!(!idle.is_empty(), "the idle streak was never reported");
    assert!(idle.iter().max().copied().unwrap_or(0) >= 2, "{idle:?}");
    assert!(
        seen.iter().any(|m| m.contains("checks_passing_since_step")),
        "the deployment was never told when the checks started passing"
    );
}

/// Stated as facts, never as a decision. Deciding the task is finished for the
/// deployment would be the harness solving it, and would make the measurement
/// meaningless.
#[test]
fn the_loop_does_not_complete_the_task_on_the_deployments_behalf() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("code.rs"), "broken").unwrap();
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    let policy = ToolPolicy {
        root: root.path().to_path_buf(),
        allow_commands: vec!["true".into()],
        output_limit: 8192,
        timeout: Duration::from_secs(5),
        sandbox: SandboxPolicy::Disabled,
        approvals: Vec::new(),
    };
    let request = ModelRequest {
        deployment: DeploymentDescriptor {
            schema_version: 1,
            id: poorai_domain::new_id(),
            provider: "fake".into(),
            endpoint: "http://localhost/".into(),
            model_ref: "fake".into(),
            backend_options: Default::default(),
            auth_ref: None,
        },
        messages: vec![],
        context_tokens: 8192,
        tools: None,
        seed: None,
        sampling: Default::default(),
    };
    let outcome =
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(poorai_orchestrator::run_action_loop(
                &store,
                &WatchingProvider {
                    turn: Arc::new(Mutex::new(0)),
                    seen: Arc::new(Mutex::new(Vec::new())),
                    hash: poorai_domain::hash_bytes("broken"),
                },
                run_id,
                request,
                &policy,
                &[("true".to_string(), vec![])],
                5,
            ));
    // The repository was fixed and completion was never declared, so the run
    // fails. Telling the deployment more did not make the loop decide for it.
    assert!(outcome.is_err());
    assert_eq!(
        std::fs::read_to_string(root.path().join("code.rs")).unwrap(),
        "fixed"
    );
    assert!(
        !store
            .events_for_run(run_id)
            .unwrap()
            .iter()
            .any(|e| e.event_type == "task.complete")
    );
}

/// A history that is the task followed by a run of tool messages answering
/// nothing leaves the deployment unable to see what it already proposed, so it
/// re-derives the same action from the same unchanged prompt. Measured: a model
/// re-sent a byte-identical edit four times across two intervening re-reads of
/// a file it had already correctly fixed.
mod history {
    use super::*;

    /// Records every history it is sent, whole.
    struct RecordingProvider {
        turn: Arc<Mutex<usize>>,
        histories: Arc<Mutex<Vec<Vec<ChatMessage>>>>,
        hash: String,
    }
    #[async_trait]
    impl ModelProvider for RecordingProvider {
        async fn inspect(
            &self,
            _: &DeploymentDescriptor,
        ) -> Result<ModelInspection, ProviderError> {
            unreachable!()
        }
        async fn runtime_state(&self) -> Result<BackendState, ProviderError> {
            unreachable!()
        }
        async fn chat(&self, request: ModelRequest) -> Result<ModelStream, ProviderError> {
            self.histories
                .lock()
                .unwrap()
                .push(request.messages.clone());
            let mut turn = self.turn.lock().unwrap();
            *turn += 1;
            let action = if *turn == 1 {
                serde_json::json!({
                    "capability": "replace_text", "path": "code.rs",
                    "expected_hash": self.hash, "find": "broken", "replace": "fixed"
                })
            } else {
                serde_json::json!({"capability": "read_file", "path": "code.rs"})
            };
            Ok(Box::pin(futures_util::stream::iter([Ok(ModelChunk {
                content: action.to_string(),
                done: true,
                ..Default::default()
            })])))
        }
    }

    fn histories() -> Vec<Vec<ChatMessage>> {
        let root = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        std::fs::write(root.path().join("code.rs"), "broken").unwrap();
        let policy = ToolPolicy {
            root: root.path().to_path_buf(),
            allow_commands: vec!["true".into()],
            output_limit: 8192,
            timeout: Duration::from_secs(5),
            sandbox: SandboxPolicy::Disabled,
            approvals: Vec::new(),
        };
        let histories = Arc::new(Mutex::new(Vec::new()));
        let request = ModelRequest {
            deployment: DeploymentDescriptor {
                schema_version: 1,
                id: poorai_domain::new_id(),
                provider: "fake".into(),
                endpoint: "http://localhost/".into(),
                model_ref: "fake".into(),
                backend_options: Default::default(),
                auth_ref: None,
            },
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "fix it".into(),
            }],
            context_tokens: 8192,
            tools: None,
            seed: None,
            sampling: Default::default(),
        };
        let _ =
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(poorai_orchestrator::run_action_loop(
                    &Store::open(":memory:").unwrap(),
                    &RecordingProvider {
                        turn: Arc::new(Mutex::new(0)),
                        histories: histories.clone(),
                        hash: poorai_domain::hash_bytes("broken"),
                    },
                    poorai_domain::new_id(),
                    request,
                    &policy,
                    &[("true".to_string(), vec![])],
                    4,
                ));
        histories.lock().unwrap().clone()
    }

    #[test]
    fn the_deployments_own_turn_is_in_the_history_it_is_sent_next() {
        let captured = histories();
        assert!(captured.len() >= 2, "the loop made only one request");
        assert!(
            captured[1].iter().any(|m| m.role == "assistant"),
            "the deployment's own turn never reached the history it was sent next"
        );
        // What it proposed is there verbatim, not summarised away.
        assert!(
            captured[1]
                .iter()
                .any(|m| m.role == "assistant" && m.content.contains("replace_text")),
            "the assistant turn does not carry the action that was proposed"
        );
    }

    /// Every tool result answers a turn rather than standing alone.
    #[test]
    fn no_tool_result_stands_without_a_turn_it_answers() {
        for messages in histories() {
            let mut assistant_turns = 0;
            let mut tool_results = 0;
            for message in &messages {
                match message.role.as_str() {
                    "assistant" => assistant_turns += 1,
                    "tool" => tool_results += 1,
                    _ => {}
                }
            }
            assert!(
                tool_results <= assistant_turns,
                "{tool_results} tool results against {assistant_turns} turns"
            );
        }
    }
}

/// A check that was failing before the deployment touched anything is a fact
/// the loop has and the deployment does not. Measured on more-itertools, whose
/// CI-declared check begins with `pip install` and therefore cannot run in a
/// sandbox with no network: it failed on every turn regardless of what the
/// deployment did, and three runs that had correctly fixed their bug were
/// recorded as failures.
mod already_failing {
    use super::*;

    struct QuietProvider;
    #[async_trait]
    impl ModelProvider for QuietProvider {
        async fn inspect(
            &self,
            _: &DeploymentDescriptor,
        ) -> Result<ModelInspection, ProviderError> {
            unreachable!()
        }
        async fn runtime_state(&self) -> Result<BackendState, ProviderError> {
            unreachable!()
        }
        async fn chat(&self, request: ModelRequest) -> Result<ModelStream, ProviderError> {
            // Answer with what we were told, so the test can read it back.
            let told = request
                .messages
                .iter()
                .map(|m| m.content.clone())
                .collect::<Vec<_>>()
                .join("\n");
            Ok(Box::pin(futures_util::stream::iter([Ok(ModelChunk {
                content: serde_json::json!({
                    "capability": "complete", "rationale": told
                })
                .to_string(),
                done: true,
                ..Default::default()
            })])))
        }
    }

    fn run_with(check: (String, Vec<String>)) -> String {
        let root = tempfile::tempdir().unwrap();
        let policy = ToolPolicy {
            root: root.path().to_path_buf(),
            allow_commands: vec!["false".into(), "true".into()],
            output_limit: 8192,
            timeout: Duration::from_secs(5),
            sandbox: SandboxPolicy::Disabled,
            approvals: Vec::new(),
        };
        let store = Store::open(":memory:").unwrap();
        let run_id = poorai_domain::new_id();
        let request = ModelRequest {
            deployment: DeploymentDescriptor {
                schema_version: 1,
                id: poorai_domain::new_id(),
                provider: "fake".into(),
                endpoint: "http://localhost/".into(),
                model_ref: "fake".into(),
                backend_options: Default::default(),
                auth_ref: None,
            },
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "fix it".into(),
            }],
            context_tokens: 8192,
            tools: None,
            seed: None,
            sampling: Default::default(),
        };
        let _ =
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(poorai_orchestrator::run_action_loop(
                    &store,
                    &QuietProvider,
                    run_id,
                    request,
                    &policy,
                    &[check],
                    3,
                ));
        store
            .events_for_run(run_id)
            .unwrap()
            .into_iter()
            .filter(|e| e.event_type == "tool.action")
            .map(|e| e.payload["action"]["rationale"].to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_deployment_is_told_which_checks_were_already_failing() {
        let told = run_with(("false".to_string(), vec![]));
        assert!(
            told.contains("checks_already_failing_before_you_started"),
            "the deployment was judged against a check it was never told was broken"
        );
        assert!(told.contains("false"), "{told}");
    }

    /// Said only when there is something to say.
    #[test]
    fn nothing_is_said_when_every_check_was_green() {
        let told = run_with(("true".to_string(), vec![]));
        assert!(
            !told.contains("checks_already_failing_before_you_started"),
            "a run with no failing checks was told there were some"
        );
    }
}
