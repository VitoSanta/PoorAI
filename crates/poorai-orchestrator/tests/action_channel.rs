//! The action channel: native tool calls, and what happens when one is denied.

use poorai_domain::ToolCall;
use poorai_orchestrator::{action_from_tool_call, action_tool_schema};
use poorai_tools::ActionProposal;

fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        name: name.into(),
        arguments,
        id: None,
    }
}

#[test]
fn the_schema_offers_exactly_the_typed_capabilities() {
    let schema = action_tool_schema();
    let names: Vec<&str> = schema
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec![
            "read_file",
            "search",
            "list_tree",
            "apply_replace",
            "run_command",
            "complete"
        ]
    );
}

#[test]
fn a_native_call_becomes_its_typed_action() {
    let action = action_from_tool_call(&call(
        "apply_replace",
        serde_json::json!({
            "path": "src/lib.rs",
            "expected_hash": "abc",
            "replacement": "fn main() {}",
        }),
    ))
    .unwrap();
    let ActionProposal::ApplyReplace { path, .. } = action else {
        panic!("wrong capability");
    };
    assert_eq!(path, "src/lib.rs");
}

/// A name outside the offered set is refused, not guessed at.
#[test]
fn an_unoffered_tool_name_is_refused() {
    for name in ["exfiltrate", "read_file_v2", "readfile", ""] {
        assert!(
            action_from_tool_call(&call(name, serde_json::json!({"path": "a"}))).is_err(),
            "accepted: {name}"
        );
    }
}

#[test]
fn arguments_that_do_not_match_the_declared_schema_are_refused() {
    // Missing a required field.
    assert!(action_from_tool_call(&call("search", serde_json::json!({"query": "x"}))).is_err());
    // Wrong type.
    assert!(
        action_from_tool_call(&call(
            "list_tree",
            serde_json::json!({"max_entries": "many"})
        ))
        .is_err()
    );
    // Not an object at all.
    assert!(action_from_tool_call(&call("list_tree", serde_json::json!([1]))).is_err());
}

/// Policy still applies after a call is typed: the tool channel carries no
/// authority of its own.
#[test]
fn a_typed_call_is_still_subject_to_validation() {
    assert!(action_from_tool_call(&call("read_file", serde_json::json!({"path": ""}))).is_err());
    assert!(
        action_from_tool_call(&call(
            "search",
            serde_json::json!({"query": "x", "max_matches": 0})
        ))
        .is_err()
    );
}

#[test]
fn a_call_naming_complete_without_a_rationale_is_refused() {
    assert!(
        action_from_tool_call(&call("complete", serde_json::json!({"rationale": ""}))).is_err()
    );
    assert!(
        action_from_tool_call(&call("complete", serde_json::json!({"rationale": "done"}))).is_ok()
    );
}

// ------------------------------------------------------- denial feedback

use async_trait::async_trait;
use poorai_domain::{
    BackendState, DeploymentDescriptor, ModelChunk, ModelInspection, ModelRequest,
};
use poorai_provider::{ModelProvider, ModelStream, ProviderError};
use poorai_store::Store;
use poorai_tools::{SandboxPolicy, ToolPolicy};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Proposes a denied edit first, then a valid one, then completes -- the shape
/// of a run whose first attempt is refused.
struct RecoveringProvider {
    turn: Arc<Mutex<usize>>,
    hash: String,
}
#[async_trait]
impl ModelProvider for RecoveringProvider {
    async fn inspect(&self, _: &DeploymentDescriptor) -> Result<ModelInspection, ProviderError> {
        unreachable!()
    }
    async fn runtime_state(&self) -> Result<BackendState, ProviderError> {
        unreachable!()
    }
    async fn chat(&self, _: ModelRequest) -> Result<ModelStream, ProviderError> {
        let mut turn = self.turn.lock().unwrap();
        *turn += 1;
        let call = match *turn {
            // A stale hash: the refusal says "reread before editing".
            1 => serde_json::json!({
                "capability": "apply_replace", "path": "code.rs",
                "expected_hash": "stale", "replacement": "fixed"
            }),
            2 => serde_json::json!({
                "capability": "apply_replace", "path": "code.rs",
                "expected_hash": self.hash, "replacement": "fixed"
            }),
            _ => serde_json::json!({"capability": "complete", "rationale": "done"}),
        };
        Ok(Box::pin(futures_util::stream::iter([Ok(ModelChunk {
            content: call.to_string(),
            done: true,
            ..Default::default()
        })])))
    }
}

/// Aborting on the first refusal discards work already done. The action
/// budget, not the first denial, is what bounds the loop.
#[tokio::test]
async fn a_denied_action_is_returned_to_the_model_rather_than_ending_the_run() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("code.rs"), "broken").unwrap();
    let policy = ToolPolicy {
        root: root.path().to_path_buf(),
        allow_commands: vec![],
        output_limit: 4096,
        timeout: Duration::from_secs(5),
        sandbox: SandboxPolicy::Disabled,
        approvals: Vec::new(),
    };
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    let provider = RecoveringProvider {
        turn: Arc::new(Mutex::new(0)),
        hash: poorai_domain::hash_bytes("broken"),
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
        context_tokens: 512,
        tools: None,
        seed: None,
        temperature_milli: None,
    };
    let result =
        poorai_orchestrator::run_action_loop(&store, &provider, run_id, request, &policy, &[], 6)
            .await
            .unwrap();
    assert!(result.verified);
    assert_eq!(
        std::fs::read_to_string(root.path().join("code.rs")).unwrap(),
        "fixed"
    );
    let actions: Vec<_> = store
        .events_for_run(run_id)
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == "tool.action")
        .collect();
    // The refusal is in the audit, and the run continued past it.
    assert_eq!(actions[0].payload["status"], "denied");
    assert_eq!(actions[1].payload["status"], "allowed");
}

/// Every event of one run shares its identifier, or the audit is split in two
/// and `report` shows only half of it.
#[tokio::test]
async fn the_whole_run_is_recorded_under_one_identifier() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("code.rs"), "broken").unwrap();
    let policy = ToolPolicy {
        root: root.path().to_path_buf(),
        allow_commands: vec![],
        output_limit: 4096,
        timeout: Duration::from_secs(5),
        sandbox: SandboxPolicy::Disabled,
        approvals: Vec::new(),
    };
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    let provider = RecoveringProvider {
        turn: Arc::new(Mutex::new(2)),
        hash: poorai_domain::hash_bytes("broken"),
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
        context_tokens: 512,
        tools: None,
        seed: None,
        temperature_milli: None,
    };
    poorai_orchestrator::run_action_loop(&store, &provider, run_id, request, &policy, &[], 4)
        .await
        .unwrap();
    let events = store.events_for_run(run_id).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.event_type == "verification.baseline")
    );
    assert!(events.iter().any(|e| e.event_type == "task.complete"));
    for pair in events.windows(2) {
        assert_eq!(pair[1].previous_hash.as_ref(), Some(&pair[0].event_hash));
    }
}

/// verification-recovery.md: "make one hypothesis-linked correction, rerun the
/// narrow check". Without the rerun the deployment cannot learn whether its
/// edit worked, so a correct edit is followed by guessing.
#[tokio::test]
async fn a_successful_edit_is_followed_by_the_narrow_check() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("code.rs"), "broken").unwrap();
    let policy = ToolPolicy {
        root: root.path().to_path_buf(),
        allow_commands: vec!["true".into()],
        output_limit: 4096,
        timeout: Duration::from_secs(10),
        sandbox: SandboxPolicy::Disabled,
        approvals: Vec::new(),
    };
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    let provider = RecoveringProvider {
        turn: Arc::new(Mutex::new(1)),
        hash: poorai_domain::hash_bytes("broken"),
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
        context_tokens: 512,
        tools: None,
        seed: None,
        temperature_milli: None,
    };
    let checks = vec![("true".to_string(), vec![])];
    poorai_orchestrator::run_action_loop(&store, &provider, run_id, request, &policy, &checks, 6)
        .await
        .unwrap();
    let interim: Vec<_> = store
        .events_for_run(run_id)
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == "verification.interim")
        .collect();
    assert_eq!(interim.len(), 1, "the edit was not followed by a check");
    assert_eq!(interim[0].payload["passing"], true);
}

/// A denied edit changed nothing, so there is nothing to re-check.
#[tokio::test]
async fn a_denied_edit_does_not_trigger_a_check() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("code.rs"), "broken").unwrap();
    let policy = ToolPolicy {
        root: root.path().to_path_buf(),
        allow_commands: vec!["true".into()],
        output_limit: 4096,
        timeout: Duration::from_secs(10),
        sandbox: SandboxPolicy::Disabled,
        approvals: Vec::new(),
    };
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    // Turn 0 proposes the stale-hash edit, which is refused.
    let provider = RecoveringProvider {
        turn: Arc::new(Mutex::new(0)),
        hash: poorai_domain::hash_bytes("broken"),
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
        context_tokens: 512,
        tools: None,
        seed: None,
        temperature_milli: None,
    };
    let checks = vec![("true".to_string(), vec![])];
    poorai_orchestrator::run_action_loop(&store, &provider, run_id, request, &policy, &checks, 6)
        .await
        .unwrap();
    let events = store.events_for_run(run_id).unwrap();
    let denied = events
        .iter()
        .filter(|e| e.payload["status"] == "denied")
        .count();
    let interim = events
        .iter()
        .filter(|e| e.event_type == "verification.interim")
        .count();
    assert_eq!(denied, 1);
    // One allowed edit followed, so exactly one check -- not two.
    assert_eq!(interim, 1);
}

/// The completion rule must be stated in both directions. A deployment told
/// only when *not* to complete has nothing connecting a passing check to the
/// action it implies.
#[test]
fn the_system_prompt_states_when_to_complete_and_when_not_to() {
    let prompt = poorai_orchestrator::AGENT_SYSTEM_PROMPT;
    assert!(prompt.contains("If they pass and the task is done, call complete"));
    assert!(prompt.contains("Do not call complete while the checks are failing"));
    // The hash guard is the most common denial in practice, so the prompt says
    // what to do about it rather than leaving it to be discovered per run.
    assert!(prompt.contains("re-read a file after editing it"));
}

/// The prompt is assembled from fragments; a missing space between two of them
/// silently changes the words the deployment reads.
#[test]
fn the_system_prompt_has_no_broken_spacing() {
    let prompt = poorai_orchestrator::AGENT_SYSTEM_PROMPT;
    assert!(!prompt.contains("  "), "double space in the prompt");
    assert!(!prompt.contains(".T") && !prompt.contains("sthe"));
}
