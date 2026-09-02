//! A deployment repeating a refused action is not short of budget.

use async_trait::async_trait;
use poorai_domain::{
    BackendState, ChatMessage, DeploymentDescriptor, ModelChunk, ModelInspection, ModelRequest,
};
use poorai_provider::{ModelProvider, ModelStream, ProviderError};
use poorai_store::Store;
use poorai_tools::{SandboxPolicy, ToolPolicy};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Proposes the same stale-hash edit forever, which is the shape measured in
/// every budget-exhausted run: the repository already fixed, the deployment
/// still editing.
struct StuckProvider {
    proposals: Arc<Mutex<usize>>,
}
#[async_trait]
impl ModelProvider for StuckProvider {
    async fn inspect(&self, _: &DeploymentDescriptor) -> Result<ModelInspection, ProviderError> {
        unreachable!()
    }
    async fn runtime_state(&self) -> Result<BackendState, ProviderError> {
        unreachable!()
    }
    async fn chat(&self, _: ModelRequest) -> Result<ModelStream, ProviderError> {
        *self.proposals.lock().unwrap() += 1;
        Ok(Box::pin(futures_util::stream::iter([Ok(ModelChunk {
            content: r#"{"capability":"replace_text","path":"code.rs","expected_hash":"stale","find":"one","replace":"two"}"#.into(),
            done: true,
            ..Default::default()
        })])))
    }
}

fn run(max_actions: u8) -> (Store, poorai_domain::Id) {
    let root = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    std::fs::write(root.path().join("code.rs"), "one").unwrap();
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
            &store,
            &StuckProvider {
                proposals: Arc::new(Mutex::new(0)),
            },
            run_id,
            request,
            &policy,
            &[],
            max_actions,
        ));
    (store, run_id)
}

#[test]
fn a_repeated_refusal_is_named_rather_than_absorbed() {
    let (store, run_id) = run(12);
    let events = store.events_for_run(run_id).unwrap();
    let detections = events
        .iter()
        .filter(|e| e.event_type == "loop.detected")
        .count();
    assert!(detections > 0, "the repetition was never named");
}

/// The point of naming it: the budget stops being spent on repeats.
#[test]
fn the_deployment_is_told_before_the_budget_is_gone() {
    let (store, run_id) = run(12);
    let events = store.events_for_run(run_id).unwrap();
    let first_detection = events
        .iter()
        .position(|e| e.event_type == "loop.detected")
        .expect("no detection");
    let attempts_before = events
        .iter()
        .take(first_detection)
        .filter(|e| e.event_type == "tool.action")
        .count();
    // Two refusals are a retry; the third is a loop. It is named well before
    // the twelve-action budget is spent.
    assert!(
        (3..=5).contains(&attempts_before),
        "named after {attempts_before} attempts"
    );
}

/// A retry after a genuine change is not a loop, so a corrected edit must not
/// be counted against the streak.
#[tokio::test]
async fn a_successful_action_clears_the_streak() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("code.rs"), "one").unwrap();
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
    // Two refusals, then a success, then two more refusals: never three in a
    // row, so never a loop.
    for hash in [
        "stale",
        "stale",
        &poorai_domain::hash_bytes("one"),
        "stale",
        "stale",
    ] {
        let _ = poorai_orchestrator::execute_action(
            &store,
            run_id,
            &policy,
            poorai_tools::ActionProposal::ReplaceText {
                path: "code.rs".into(),
                expected_hash: hash.to_string(),
                find: "one".into(),
                replace: "two".into(),
            },
        )
        .await;
    }
    let denials = store
        .events_for_run(run_id)
        .unwrap()
        .iter()
        .filter(|e| e.payload["status"] == "denied")
        .count();
    assert_eq!(denials, 4);
}
