//! A plan that is carried rather than mentioned.
//!
//! A plan pushed once as a message is context, not authority: nothing consults
//! it again, and compaction drops it entirely — so on a long task the
//! decomposition disappears exactly when it starts to matter. Held as loop
//! state it survives compaction, appears in the status of every turn, and is
//! reconciled when completion is declared.

use async_trait::async_trait;
use poorai_domain::{
    BackendState, ChatMessage, DeploymentDescriptor, ModelChunk, ModelInspection, ModelRequest,
    ToolCall,
};
use poorai_provider::{ModelProvider, ModelStream, ProviderError};
use poorai_store::Store;
use poorai_tools::{SandboxPolicy, ToolPolicy};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Plans three steps, records the first two as done, then completes.
struct PlanningProvider {
    turn: Arc<Mutex<usize>>,
    seen: Arc<Mutex<Vec<Vec<ChatMessage>>>>,
}
#[async_trait]
impl ModelProvider for PlanningProvider {
    async fn inspect(&self, _: &DeploymentDescriptor) -> Result<ModelInspection, ProviderError> {
        unreachable!()
    }
    async fn runtime_state(&self) -> Result<BackendState, ProviderError> {
        unreachable!()
    }
    async fn chat(&self, request: ModelRequest) -> Result<ModelStream, ProviderError> {
        self.seen.lock().unwrap().push(request.messages.clone());
        let mut turn = self.turn.lock().unwrap();
        *turn += 1;
        let call = |name: &str, arguments: serde_json::Value| ModelChunk {
            content: String::new(),
            done: true,
            tool_calls: vec![ToolCall {
                name: name.into(),
                arguments,
                id: None,
            }],
            ..Default::default()
        };
        let chunk = match *turn {
            1 => call(
                "plan",
                serde_json::json!({"steps": ["read the file", "fix the bug", "add a test"]}),
            ),
            2 => call("record_progress", serde_json::json!({"step": 1})),
            3 => call("record_progress", serde_json::json!({"step": 2})),
            _ => call("complete", serde_json::json!({"rationale": "done"})),
        };
        Ok(Box::pin(futures_util::stream::iter([Ok(chunk)])))
    }
}

fn run() -> (Store, poorai_domain::Id, Vec<Vec<ChatMessage>>) {
    let root = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    std::fs::write(root.path().join("code.rs"), "body").unwrap();
    let policy = ToolPolicy {
        root: root.path().to_path_buf(),
        extra_readable: Vec::new(),
        allow_commands: vec!["true".into()],
        output_limit: 8192,
        timeout: Duration::from_secs(5),
        sandbox: SandboxPolicy::Disabled,
        approvals: Vec::new(),
    };
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
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
            ..Default::default()
        }],
        context_tokens: 8192,
        tools: None,
        seed: None,
        sampling: Default::default(),
    };
    let _ = tokio::runtime::Runtime::new().unwrap().block_on(
        poorai_orchestrator::run_action_loop_with_prompt(
            &store,
            &PlanningProvider {
                turn: Arc::new(Mutex::new(0)),
                seen: seen.clone(),
            },
            run_id,
            request,
            &policy,
            &[("true".to_string(), vec![])],
            6,
            &poorai_orchestrator::DenyWithoutAsking,
            true,
        ),
    );
    let histories = seen.lock().unwrap().clone();
    (store, run_id, histories)
}

fn payloads(store: &Store, run_id: poorai_domain::Id, kind: &str) -> Vec<serde_json::Value> {
    store
        .events_for_run(run_id)
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == kind)
        .map(|e| e.payload)
        .collect()
}

/// Every turn carries what remains, because a step the deployment can no
/// longer read is not a plan.
#[test]
fn the_outstanding_steps_are_repeated_in_the_status_of_every_turn() {
    let (_, _, histories) = run();
    // The turn after the first step was recorded.
    let after_first = histories
        .iter()
        .find(|messages| {
            messages
                .iter()
                .any(|m| m.role == "tool" && m.content.contains("\"plan_steps_done\":1"))
        })
        .expect("no turn reported one step done");
    let status = after_first
        .iter()
        .rev()
        .find(|m| m.role == "tool" && m.content.contains("plan_steps_outstanding"))
        .expect("outstanding steps were never reported");
    assert!(
        status.content.contains("2. fix the bug"),
        "{}",
        status.content
    );
    assert!(
        status.content.contains("3. add a test"),
        "{}",
        status.content
    );
    // The step already done is not offered back as outstanding.
    assert!(
        !status.content.contains("1. read the file"),
        "a finished step was still listed as outstanding: {}",
        status.content
    );
}

/// The claim is the deployment's; the harness never infers it.
#[test]
fn progress_is_recorded_from_the_deployments_own_claim() {
    let (store, run_id, _) = run();
    let recorded: Vec<_> = payloads(&store, run_id, "tool.action")
        .into_iter()
        .filter(|p| p["action"]["capability"] == "record_progress")
        .collect();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0]["action"]["step"], 1);
    assert_eq!(recorded[1]["action"]["step"], 2);
}

/// Recorded, not enforced: a plan is explicitly not binding and can be wrong,
/// so a completion with steps outstanding is a fact to preserve rather than a
/// reason to refuse.
#[test]
fn completion_is_reconciled_against_the_plan_without_being_blocked_by_it() {
    let (store, run_id, _) = run();
    let reconciled = payloads(&store, run_id, "plan.reconciled");
    assert_eq!(reconciled.len(), 1);
    assert_eq!(reconciled[0]["steps_total"], 3);
    assert_eq!(reconciled[0]["steps_recorded_done"], 2);
    assert_eq!(
        reconciled[0]["steps_outstanding"],
        serde_json::json!(["3. add a test"])
    );
    // The run still completed: the plan did not veto it.
    assert_eq!(payloads(&store, run_id, "task.complete").len(), 1);
}

/// A claim on a step the plan does not have is a mistake, not progress.
#[test]
fn a_claim_beyond_the_plan_is_not_counted_as_progress() {
    struct OverclaimingProvider {
        turn: Arc<Mutex<usize>>,
    }
    #[async_trait]
    impl ModelProvider for OverclaimingProvider {
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
            let mut turn = self.turn.lock().unwrap();
            *turn += 1;
            let call = |name: &str, arguments: serde_json::Value| ModelChunk {
                content: String::new(),
                done: true,
                tool_calls: vec![ToolCall {
                    name: name.into(),
                    arguments,
                    id: None,
                }],
                ..Default::default()
            };
            let chunk = match *turn {
                1 => call("plan", serde_json::json!({"steps": ["only step"]})),
                2 => call("record_progress", serde_json::json!({"step": 9})),
                _ => call("complete", serde_json::json!({"rationale": "done"})),
            };
            Ok(Box::pin(futures_util::stream::iter([Ok(chunk)])))
        }
    }
    let root = tempfile::tempdir().unwrap();
    let policy = ToolPolicy {
        root: root.path().to_path_buf(),
        extra_readable: Vec::new(),
        allow_commands: vec!["true".into()],
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
            ..Default::default()
        }],
        context_tokens: 8192,
        tools: None,
        seed: None,
        sampling: Default::default(),
    };
    let _ = tokio::runtime::Runtime::new().unwrap().block_on(
        poorai_orchestrator::run_action_loop_with_prompt(
            &store,
            &OverclaimingProvider {
                turn: Arc::new(Mutex::new(0)),
            },
            run_id,
            request,
            &policy,
            &[("true".to_string(), vec![])],
            5,
            &poorai_orchestrator::DenyWithoutAsking,
            true,
        ),
    );
    let reconciled = payloads(&store, run_id, "plan.reconciled");
    assert_eq!(reconciled[0]["steps_recorded_done"], 0);
    assert_eq!(
        reconciled[0]["steps_outstanding"],
        serde_json::json!(["1. only step"])
    );
}

/// Compaction is exactly when a long task most needs its plan: the message
/// that carried it is among the first things dropped. A decomposition that
/// disappears at the checkpoint is the defect this item was opened for.
#[test]
fn the_plan_survives_compaction() {
    /// Plans, records one step, then reads a large file until the history is
    /// compacted, so a run reaches the checkpoint with a plan still in force.
    struct BulkyProvider {
        turn: Arc<Mutex<usize>>,
        seen: Arc<Mutex<Vec<Vec<ChatMessage>>>>,
    }
    #[async_trait]
    impl ModelProvider for BulkyProvider {
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
            self.seen.lock().unwrap().push(request.messages.clone());
            let mut turn = self.turn.lock().unwrap();
            *turn += 1;
            let call = |name: &str, arguments: serde_json::Value| ModelChunk {
                content: String::new(),
                done: true,
                tool_calls: vec![ToolCall {
                    name: name.into(),
                    arguments,
                    id: None,
                }],
                ..Default::default()
            };
            let chunk = match *turn {
                1 => call(
                    "plan",
                    serde_json::json!({"steps": ["read it", "fix it", "test it"]}),
                ),
                2 => call("record_progress", serde_json::json!({"step": 1})),
                _ => call("read_file", serde_json::json!({"path": "big.txt"})),
            };
            Ok(Box::pin(futures_util::stream::iter([Ok(chunk)])))
        }
    }

    let root = tempfile::tempdir().unwrap();
    // Large enough that reading it twice exceeds the history budget.
    std::fs::write(root.path().join("big.txt"), "x".repeat(40_000)).unwrap();
    let policy = ToolPolicy {
        root: root.path().to_path_buf(),
        extra_readable: Vec::new(),
        allow_commands: vec!["true".into()],
        output_limit: 64 * 1024,
        timeout: Duration::from_secs(5),
        sandbox: SandboxPolicy::Disabled,
        approvals: Vec::new(),
    };
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
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
            ..Default::default()
        }],
        context_tokens: 8192,
        tools: None,
        seed: None,
        sampling: Default::default(),
    };
    let _ = tokio::runtime::Runtime::new().unwrap().block_on(
        poorai_orchestrator::run_action_loop_with_prompt(
            &store,
            &BulkyProvider {
                turn: Arc::new(Mutex::new(0)),
                seen: seen.clone(),
            },
            run_id,
            request,
            &policy,
            &[("true".to_string(), vec![])],
            8,
            &poorai_orchestrator::DenyWithoutAsking,
            true,
        ),
    );
    let histories = seen.lock().unwrap().clone();
    assert!(
        !payloads(&store, run_id, "context.compacted").is_empty(),
        "the history was never compacted, so this proves nothing"
    );
    // Every request built after the first compaction still carries the plan.
    let compacted_at = histories
        .iter()
        .position(|messages| {
            messages
                .iter()
                .any(|m| m.content.contains("Ledger of this run so far"))
        })
        .expect("no history was rebuilt from the ledger");
    for messages in &histories[compacted_at..] {
        let carries_plan = messages
            .iter()
            .any(|m| m.content.contains("Your plan, still in force"));
        assert!(
            carries_plan,
            "a request after compaction carried no plan: {:?}",
            messages.iter().map(|m| &m.role).collect::<Vec<_>>()
        );
    }
    // And it carries the progress, not just the steps.
    let after = &histories[compacted_at];
    let plan_message = after
        .iter()
        .find(|m| m.content.contains("Your plan, still in force"))
        .unwrap();
    assert!(
        plan_message.content.contains("Done: 1 of 3"),
        "{}",
        plan_message.content
    );
    assert!(
        plan_message.content.contains("2. fix it"),
        "{}",
        plan_message.content
    );
    assert!(
        !plan_message.content.contains("1. read it"),
        "a finished step was carried as outstanding: {}",
        plan_message.content
    );
}

/// A claim the harness can check is a claim it should check.
///
/// The boundary does not move: the harness still never *infers* that a step is
/// done. It tests a claim against a command, which is what it already does for
/// completion -- and a step whose check did not pass is not done however
/// loudly it was claimed.
mod subgoal_checks {
    use poorai_orchestrator::plan::{Plan, Subgoal};

    fn plan_with_check(command: Option<Vec<String>>) -> Plan {
        Plan::new(vec![
            Subgoal {
                statement: "write the parser".into(),
                ..Default::default()
            },
            Subgoal {
                statement: "make the tests pass".into(),
                depends_on: vec![1],
                verify: command,
            },
        ])
    }

    #[test]
    fn a_claim_on_a_checked_step_is_not_done_until_the_check_passes() {
        let mut plan = plan_with_check(Some(vec!["cargo".into(), "test".into()]));
        plan.claim(2);
        // Claimed, and the check has not run: not yet contradicted, so not yet
        // refused either.
        assert!(plan.done(2));

        plan.record_verification(2, false);
        assert!(!plan.done(2), "a failed check left the step done");
        assert!(plan.outstanding().iter().any(|s| s.starts_with("2.")));

        plan.record_verification(2, true);
        assert!(plan.done(2));
    }

    /// Absent is not the same as failed. A plan without checks must not look
    /// like a plan that failed them.
    #[test]
    fn a_step_without_a_check_is_done_when_it_is_claimed() {
        let mut plan = plan_with_check(None);
        plan.claim(2);
        assert!(plan.done(2));
        assert_eq!(plan.state[1].verified, None);
    }

    #[test]
    fn a_step_waits_for_what_it_depends_on() {
        let mut plan = plan_with_check(None);
        assert_eq!(plan.ready(), vec![1]);
        assert_eq!(plan.blocked(), vec![2]);
        plan.claim(1);
        assert_eq!(plan.ready(), vec![2]);
        assert!(plan.blocked().is_empty());
    }

    /// A plan that refers to a step it does not have is a mistake in the plan,
    /// and blocking on it would strand the run over a typo.
    #[test]
    fn a_dependency_that_does_not_exist_does_not_block() {
        let plan = Plan::new(vec![Subgoal {
            statement: "do it".into(),
            depends_on: vec![9],
            verify: None,
        }]);
        assert_eq!(plan.ready(), vec![1]);
    }

    /// A claim on a step the plan does not have is a mistake, not progress --
    /// the rule the loop already applied, kept where the plan itself can hold
    /// it.
    #[test]
    fn a_claim_beyond_the_plan_is_refused() {
        let mut plan = plan_with_check(None);
        assert!(!plan.claim(9));
        assert!(!plan.claim(0));
        assert_eq!(plan.done_count(), 0);
    }
}
