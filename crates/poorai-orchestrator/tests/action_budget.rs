//! The budget counts actions, not turns.
//!
//! A malformed call performs nothing and is already bounded by
//! `MALFORMED_CALL_LIMIT`. Charging it against the action budget spends the
//! run's capacity to do work on the deployment's spelling. Measured: a run that
//! had finished its task lost two of eight actions to schema mistakes, had no
//! turn left to declare completion, and was recorded as a failure over a
//! repository whose checks were passing.

use async_trait::async_trait;
use poorai_domain::{
    BackendState, ChatMessage, DeploymentDescriptor, ModelChunk, ModelInspection, ModelRequest,
};
use poorai_provider::{ModelProvider, ModelStream, ProviderError};
use poorai_store::Store;
use poorai_tools::{SandboxPolicy, ToolPolicy};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Emits `malformed` unusable replies, then reads a file on every later turn.
struct StammeringProvider {
    turn: Arc<Mutex<usize>>,
    malformed: usize,
}
#[async_trait]
impl ModelProvider for StammeringProvider {
    async fn inspect(&self, _: &DeploymentDescriptor) -> Result<ModelInspection, ProviderError> {
        unreachable!()
    }
    async fn runtime_state(&self) -> Result<BackendState, ProviderError> {
        unreachable!()
    }
    async fn chat(&self, _: ModelRequest) -> Result<ModelStream, ProviderError> {
        let mut turn = self.turn.lock().unwrap();
        *turn += 1;
        // Alternate: one unusable reply, then one real action, so the
        // malformed calls never reach the consecutive limit.
        let content = if *turn <= self.malformed * 2 && !(*turn).is_multiple_of(2) {
            "I will now look at the file.".to_string()
        } else {
            serde_json::json!({"capability": "read_file", "path": "code.rs"}).to_string()
        };
        Ok(Box::pin(futures_util::stream::iter([Ok(ModelChunk {
            content,
            done: true,
            ..Default::default()
        })])))
    }
}

fn run(malformed: usize, max_actions: u8) -> (Store, poorai_domain::Id) {
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
            content: "look".into(),
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
                &StammeringProvider {
                    turn: Arc::new(Mutex::new(0)),
                    malformed,
                },
                run_id,
                request,
                &policy,
                &[("true".to_string(), vec![])],
                max_actions,
            ));
    let _ = outcome;
    (store, run_id)
}

fn count(store: &Store, run_id: poorai_domain::Id, event_type: &str) -> usize {
    store
        .events_for_run(run_id)
        .unwrap()
        .iter()
        .filter(|e| e.event_type == event_type)
        .count()
}

/// The measured case: two unusable replies must not cost two actions.
#[test]
fn a_malformed_call_does_not_spend_an_action() {
    let (store, run_id) = run(2, 5);
    assert_eq!(count(&store, run_id, "action.malformed"), 2);
    // All five actions were still available for work.
    assert_eq!(
        count(&store, run_id, "tool.action"),
        5,
        "malformed calls were charged against the action budget"
    );
}

/// The case the turn ceiling exists for: two unusable replies for every real
/// action, which never reaches three in a row and so is never caught by
/// `MALFORMED_CALL_LIMIT`. Without a ceiling this burns three turns per action
/// indefinitely; the consecutive limit cannot see it.
#[test]
fn a_deployment_stammering_below_the_consecutive_limit_is_still_bounded() {
    struct TwoInThreeProvider {
        turn: Arc<Mutex<usize>>,
    }
    #[async_trait]
    impl ModelProvider for TwoInThreeProvider {
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
            let content = if (*turn).is_multiple_of(3) {
                serde_json::json!({"capability": "read_file", "path": "code.rs"}).to_string()
            } else {
                "still thinking".to_string()
            };
            Ok(Box::pin(futures_util::stream::iter([Ok(ModelChunk {
                content,
                done: true,
                ..Default::default()
            })])))
        }
    }
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("code.rs"), "body").unwrap();
    let policy = ToolPolicy {
        root: root.path().to_path_buf(),
        extra_readable: Vec::new(),
        allow_commands: vec![],
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
            content: "look".into(),
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
                &TwoInThreeProvider {
                    turn: Arc::new(Mutex::new(0)),
                },
                run_id,
                request,
                &policy,
                &[],
                8,
            ));
    let error = outcome.unwrap_err();
    assert!(
        error.contains("not emitting usable calls"),
        "stopped for the wrong reason: {error}"
    );
    // It is stopped by the ceiling, having done fewer actions than its budget.
    assert!(count(&store, run_id, "tool.action") < 8);
}

/// A turn that performs nothing is still bounded, or a deployment that never
/// emits a usable call would run until the provider timed out.
#[test]
fn a_deployment_that_never_calls_anything_is_stopped() {
    struct MuteProvider;
    #[async_trait]
    impl ModelProvider for MuteProvider {
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
            Ok(Box::pin(futures_util::stream::iter([Ok(ModelChunk {
                content: "thinking about it".into(),
                done: true,
                ..Default::default()
            })])))
        }
    }
    let root = tempfile::tempdir().unwrap();
    let policy = ToolPolicy {
        root: root.path().to_path_buf(),
        extra_readable: Vec::new(),
        allow_commands: vec![],
        output_limit: 8192,
        timeout: Duration::from_secs(5),
        sandbox: SandboxPolicy::Disabled,
        approvals: Vec::new(),
    };
    let store = Store::open(":memory:").unwrap();
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
            content: "look".into(),
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
                &MuteProvider,
                poorai_domain::new_id(),
                request,
                &policy,
                &[],
                8,
            ));
    assert!(outcome.is_err(), "an endless run was allowed to continue");
}
