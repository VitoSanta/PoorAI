//! A malformed tool call is a mistake the deployment can correct.

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

/// Emits `bad` malformed calls, then a valid edit, then completes.
struct MalformingProvider {
    turn: Arc<Mutex<usize>>,
    bad: usize,
    hash: String,
}
#[async_trait]
impl ModelProvider for MalformingProvider {
    async fn inspect(&self, _: &DeploymentDescriptor) -> Result<ModelInspection, ProviderError> {
        unreachable!()
    }
    async fn runtime_state(&self) -> Result<BackendState, ProviderError> {
        unreachable!()
    }
    async fn chat(&self, _: ModelRequest) -> Result<ModelStream, ProviderError> {
        let mut turn = self.turn.lock().unwrap();
        *turn += 1;
        let chunk = if *turn <= self.bad {
            // The observed shape: the right tool named, the wrong arguments.
            ModelChunk {
                tool_calls: vec![ToolCall {
                    name: "run_command".into(),
                    arguments: serde_json::json!({"command": "cargo test"}),
                    id: None,
                }],
                done: true,
                ..Default::default()
            }
        } else if *turn == self.bad + 1 {
            ModelChunk {
                tool_calls: vec![ToolCall {
                    name: "replace_text".into(),
                    arguments: serde_json::json!({
                        "path": "code.rs",
                        "expected_hash": self.hash,
                        "find": "one",
                        "replace": "two",
                    }),
                    id: None,
                }],
                done: true,
                ..Default::default()
            }
        } else {
            ModelChunk {
                tool_calls: vec![ToolCall {
                    name: "complete".into(),
                    arguments: serde_json::json!({"rationale": "done"}),
                    id: None,
                }],
                done: true,
                ..Default::default()
            }
        };
        Ok(Box::pin(futures_util::stream::iter([Ok(chunk)])))
    }
}

fn run(bad: usize, max_actions: u8) -> (Store, poorai_domain::Id, String, Result<(), String>) {
    let root = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    std::fs::write(root.path().join("code.rs"), "one").unwrap();
    let policy = ToolPolicy {
        root: root.path().to_path_buf(),
        extra_readable: Vec::new(),
        allow_commands: vec!["true".into()],
        output_limit: 4096,
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
    let outcome =
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(poorai_orchestrator::run_action_loop(
                &store,
                &MalformingProvider {
                    turn: Arc::new(Mutex::new(0)),
                    bad,
                    hash: poorai_domain::hash_bytes("one"),
                },
                run_id,
                request,
                &policy,
                &[("true".into(), Vec::new())],
                max_actions,
            ));
    let after = std::fs::read_to_string(root.path().join("code.rs")).unwrap();
    (store, run_id, after, outcome.map(|_| ()))
}

/// Five of thirteen measured runs died on this, three of them the whole
/// generation suite. The work was reachable and the run ended before it.
#[test]
fn a_malformed_call_is_returned_to_the_deployment_and_the_run_continues() {
    let (store, run_id, after, outcome) = run(2, 8);
    assert!(outcome.is_ok(), "the run ended on a correctable mistake");
    assert_eq!(after, "two", "the edit after the mistakes never happened");
    let told = store
        .events_for_run(run_id)
        .unwrap()
        .iter()
        .filter(|e| e.event_type == "action.malformed")
        .count();
    assert_eq!(told, 2, "the deployment was not told what was wrong");
}

/// A deployment that cannot form a valid call after being told three times is
/// not going to, and the budget is better spent failing.
#[test]
fn repeated_malformed_calls_still_end_the_run() {
    let (store, run_id, _, outcome) = run(20, 30);
    assert!(outcome.is_err());
    let told = store
        .events_for_run(run_id)
        .unwrap()
        .iter()
        .filter(|e| e.event_type == "action.malformed")
        .count();
    // Told, then told again, then given up on -- not once, and not forever.
    assert_eq!(told, 4);
}
