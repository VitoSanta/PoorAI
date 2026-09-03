//! Every tool attempt must reach the audit log, allowed or denied.
//!
//! An audit that records only successes cannot show that the policy ever
//! refused anything, which is precisely what an audit exists to show.

use poorai_store::Store;
use poorai_tools::{ActionProposal, ToolPolicy};
use std::fs;
use std::path::Path;
use std::time::Duration;

fn policy(root: &Path) -> ToolPolicy {
    ToolPolicy {
        root: root.to_path_buf(),
        allow_commands: vec![],
        output_limit: 4096,
        timeout: Duration::from_secs(5),
        sandbox: poorai_tools::SandboxPolicy::Disabled,
        approvals: Vec::new(),
    }
}

fn audited(store: &Store, run_id: poorai_domain::Id) -> Vec<serde_json::Value> {
    store
        .events_for_run(run_id)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "tool.action")
        .map(|event| event.payload)
        .collect()
}

#[tokio::test]
async fn a_denied_traversal_is_recorded_with_its_denial() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    let action = ActionProposal::ReadFile {
        path: "../../etc/passwd".into(),
        first_line: None,
        max_lines: None,
    };
    let result =
        poorai_orchestrator::execute_action(&store, run_id, &policy(root.path()), action).await;
    assert!(result.is_err());
    let events = audited(&store, run_id);
    assert_eq!(events.len(), 1, "a refused action left no audit record");
    assert_eq!(events[0]["status"], "denied");
    assert_eq!(events[0]["action"]["path"], "../../etc/passwd");
    assert!(events[0]["denial"].as_str().is_some());
}

#[tokio::test]
async fn a_denied_command_is_recorded_with_the_executable_it_asked_for() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    let action = ActionProposal::RunCommand {
        executable: "curl".into(),
        args: vec!["https://example.invalid".into()],
        stdin: None,
    };
    assert!(
        poorai_orchestrator::execute_action(&store, run_id, &policy(root.path()), action)
            .await
            .is_err()
    );
    let events = audited(&store, run_id);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["status"], "denied");
    assert_eq!(events[0]["action"]["executable"], "curl");
}

#[tokio::test]
async fn a_malformed_action_is_recorded_rather_than_dropped() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    // Fails validation before any tool runs; it must still be auditable.
    let action = ActionProposal::Search {
        query: String::new(),
        max_matches: 0,
    };
    assert!(
        poorai_orchestrator::execute_action(&store, run_id, &policy(root.path()), action)
            .await
            .is_err()
    );
    let events = audited(&store, run_id);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["status"], "denied");
}

#[tokio::test]
async fn an_allowed_action_is_recorded_with_its_outcome() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("code.rs"), "fn one() {}").unwrap();
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    let action = ActionProposal::ReadFile {
        path: "code.rs".into(),
        first_line: None,
        max_lines: None,
    };
    assert!(
        poorai_orchestrator::execute_action(&store, run_id, &policy(root.path()), action)
            .await
            .is_ok()
    );
    let events = audited(&store, run_id);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["status"], "allowed");
    assert_eq!(events[0]["outcome"]["path"], "code.rs");
}

#[tokio::test]
async fn a_stale_edit_is_recorded_as_denied_and_changes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("code.rs");
    fs::write(&file, "fn one() {}").unwrap();
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    let action = ActionProposal::ApplyReplace {
        path: "code.rs".into(),
        expected_hash: poorai_domain::hash_bytes("something else"),
        replacement: "fn two() {}".into(),
    };
    assert!(
        poorai_orchestrator::execute_action(&store, run_id, &policy(root.path()), action)
            .await
            .is_err()
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "fn one() {}");
    assert_eq!(audited(&store, run_id)[0]["status"], "denied");
}

/// The log is hash-chained, so a denial cannot be excised without breaking it.
#[tokio::test]
async fn the_audit_chain_covers_denied_actions() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("code.rs"), "fn one() {}").unwrap();
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    for action in [
        ActionProposal::ReadFile {
            path: "code.rs".into(),
            first_line: None,
            max_lines: None,
        },
        ActionProposal::ReadFile {
            path: "../escape".into(),
            first_line: None,
            max_lines: None,
        },
        ActionProposal::ReadFile {
            path: "code.rs".into(),
            first_line: None,
            max_lines: None,
        },
    ] {
        let _ =
            poorai_orchestrator::execute_action(&store, run_id, &policy(root.path()), action).await;
    }
    let events = store.events_for_run(run_id).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events
            .iter()
            .filter(|e| e.payload["status"] == "denied")
            .count(),
        1
    );
    for pair in events.windows(2) {
        assert_eq!(
            pair[1].previous_hash.as_ref(),
            Some(&pair[0].event_hash),
            "denial broke the audit chain"
        );
    }
}

/// A completion is an action. Auditing every other action but not this one
/// leaves the declared rationale -- the only part of a completion that says
/// anything -- out of the record.
#[tokio::test]
async fn a_declared_completion_is_audited_with_its_rationale() {
    struct CompletingProvider;
    #[async_trait::async_trait]
    impl poorai_provider::ModelProvider for CompletingProvider {
        async fn inspect(
            &self,
            _: &poorai_domain::DeploymentDescriptor,
        ) -> Result<poorai_domain::ModelInspection, poorai_provider::ProviderError> {
            unreachable!()
        }
        async fn runtime_state(
            &self,
        ) -> Result<poorai_domain::BackendState, poorai_provider::ProviderError> {
            unreachable!()
        }
        async fn chat(
            &self,
            _: poorai_domain::ModelRequest,
        ) -> Result<poorai_provider::ModelStream, poorai_provider::ProviderError> {
            Ok(Box::pin(futures_util::stream::iter([Ok(
                poorai_domain::ModelChunk {
                    content: r#"{"capability":"complete","rationale":"checksum_of computes it"}"#
                        .into(),
                    done: true,
                    ..Default::default()
                },
            )])))
        }
    }
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    let request = poorai_domain::ModelRequest {
        deployment: poorai_domain::DeploymentDescriptor {
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
        sampling: Default::default(),
    };
    poorai_orchestrator::run_action_loop(
        &store,
        &CompletingProvider,
        run_id,
        request,
        &policy(root.path()),
        &[],
        4,
    )
    .await
    .unwrap();
    let completion = audited(&store, run_id)
        .into_iter()
        .find(|p| p["action"]["capability"] == "complete")
        .expect("the completion was not audited");
    assert_eq!(completion["action"]["rationale"], "checksum_of computes it");
}
