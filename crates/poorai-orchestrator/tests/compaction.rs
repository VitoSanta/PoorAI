//! Context compaction: what survives, and where it comes from.

use async_trait::async_trait;
use poorai_domain::{
    BackendState, ChatMessage, DeploymentDescriptor, ModelChunk, ModelInspection, ModelRequest,
};
use poorai_provider::{ModelProvider, ModelStream, ProviderError};
use poorai_store::Store;
use poorai_tools::{SandboxPolicy, ToolPolicy};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn policy(root: &Path) -> ToolPolicy {
    ToolPolicy {
        root: root.to_path_buf(),
        allow_commands: vec![],
        output_limit: 64 * 1024,
        timeout: Duration::from_secs(5),
        sandbox: SandboxPolicy::Disabled,
        approvals: Vec::new(),
    }
}

fn deployment() -> DeploymentDescriptor {
    DeploymentDescriptor {
        schema_version: 1,
        id: poorai_domain::new_id(),
        provider: "fake".into(),
        endpoint: "http://localhost/".into(),
        model_ref: "fake".into(),
        backend_options: Default::default(),
        auth_ref: None,
    }
}

/// Reads the file, edits it, then keeps reading. The reads return large
/// content, which is what drives the history past its budget.
struct ChattyProvider {
    turn: Arc<Mutex<usize>>,
    hash: String,
}
#[async_trait]
impl ModelProvider for ChattyProvider {
    async fn inspect(&self, _: &DeploymentDescriptor) -> Result<ModelInspection, ProviderError> {
        unreachable!()
    }
    async fn runtime_state(&self) -> Result<BackendState, ProviderError> {
        unreachable!()
    }
    async fn chat(&self, _: ModelRequest) -> Result<ModelStream, ProviderError> {
        let mut turn = self.turn.lock().unwrap();
        *turn += 1;
        let action = match *turn {
            1 => serde_json::json!({"capability": "read_file", "path": "big.txt"}),
            2 => serde_json::json!({
                "capability": "replace_text", "path": "big.txt",
                "expected_hash": self.hash, "find": "MARKER", "replace": "FIXED"
            }),
            _ => serde_json::json!({"capability": "read_file", "path": "big.txt"}),
        };
        Ok(Box::pin(futures_util::stream::iter([Ok(ModelChunk {
            content: action.to_string(),
            done: true,
            ..Default::default()
        })])))
    }
}

fn run_until_compaction(root: &Path) -> (Store, poorai_domain::Id, ModelRequest) {
    let body = format!("{}\nMARKER\n", "filler line\n".repeat(400));
    std::fs::write(root.join("big.txt"), &body).unwrap();
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    let request = ModelRequest {
        deployment: deployment(),
        messages: vec![
            ChatMessage {
                role: "system".into(),
                content: poorai_orchestrator::AGENT_SYSTEM_PROMPT.into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "replace MARKER".into(),
            },
        ],
        // Small enough that a few large reads exceed half of it.
        context_tokens: 2048,
        tools: None,
        seed: None,
        temperature_milli: None,
    };
    let provider = ChattyProvider {
        turn: Arc::new(Mutex::new(0)),
        hash: poorai_domain::hash_bytes(&body),
    };
    let _ = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(poorai_orchestrator::run_action_loop(
            &store,
            &provider,
            run_id,
            request.clone(),
            &policy(root),
            &[],
            6,
        ));
    (store, run_id, request)
}

#[test]
fn a_long_history_is_compacted_at_a_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let (store, run_id, _) = run_until_compaction(root.path());
    let events = store.events_for_run(run_id).unwrap();
    let compactions: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "context.compacted")
        .collect();
    assert!(!compactions.is_empty(), "history was never compacted");
    let first = &compactions[0].payload;
    assert!(
        first["estimated_tokens_after"].as_u64() < first["estimated_tokens_before"].as_u64(),
        "compaction did not shrink the history"
    );
    // The estimate is labelled as one rather than presented as a count.
    assert!(
        first["estimate_basis"]
            .as_str()
            .is_some_and(|b| b.contains("not a provider count"))
    );
}

/// The ledger is built from the audit, not from the deployment's recollection:
/// a summary the model wrote could be wrong about what it did.
#[test]
fn the_ledger_carries_hashes_and_refusals_from_the_audit() {
    let root = tempfile::tempdir().unwrap();
    let body = "one\nMARKER\ntwo\n";
    std::fs::write(root.path().join("big.txt"), body).unwrap();
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    // A read, a refused edit, then an accepted one.
    runtime.block_on(async {
        let _ = poorai_orchestrator::execute_action(
            &store,
            run_id,
            &policy(root.path()),
            poorai_tools::ActionProposal::ReadFile {
                path: "big.txt".into(),
                first_line: None,
                max_lines: None,
            },
        )
        .await;
        let _ = poorai_orchestrator::execute_action(
            &store,
            run_id,
            &policy(root.path()),
            poorai_tools::ActionProposal::ReplaceText {
                path: "big.txt".into(),
                expected_hash: "stale".into(),
                find: "MARKER".into(),
                replace: "X".into(),
            },
        )
        .await;
        let _ = poorai_orchestrator::execute_action(
            &store,
            run_id,
            &policy(root.path()),
            poorai_tools::ActionProposal::ReplaceText {
                path: "big.txt".into(),
                expected_hash: poorai_domain::hash_bytes(body),
                find: "MARKER".into(),
                replace: "FIXED".into(),
            },
        )
        .await;
    });
    let ledger = poorai_orchestrator::task_ledger(&store, run_id).unwrap();
    assert!(ledger.contains("big.txt"), "the file is not in the ledger");
    // The hash after the edit, so an edit planned before compaction still works.
    let after = std::fs::read_to_string(root.path().join("big.txt")).unwrap();
    assert!(ledger.contains(&poorai_domain::hash_bytes(&after)));
    // A refusal is kept so it is not retried from a blank memory.
    assert!(ledger.contains("do not repeat"));
    assert!(ledger.contains("stale file hash"));
}
