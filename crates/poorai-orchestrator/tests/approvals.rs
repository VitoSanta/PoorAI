//! Interactive approval: what is asked, what a grant covers, and what happens
//! when nobody is there.

use async_trait::async_trait;
use poorai_orchestrator::{ApprovalDecision, ApprovalPrompt, DenyWithoutAsking};
use poorai_tools::{ActionProposal, Approval, required_approval};
use std::sync::{Arc, Mutex};

/// Records what it was asked and answers from a script.
struct ScriptedPrompt {
    answers: Mutex<Vec<ApprovalDecision>>,
    asked: Arc<Mutex<Vec<(Approval, String)>>>,
}
#[async_trait]
impl ApprovalPrompt for ScriptedPrompt {
    async fn ask(&self, approval: Approval, description: &str) -> ApprovalDecision {
        self.asked
            .lock()
            .unwrap()
            .push((approval, description.to_string()));
        self.answers
            .lock()
            .unwrap()
            .pop()
            .unwrap_or(ApprovalDecision::Deny)
    }
}

/// "Allow network access" tells a person nothing they can judge. The command
/// does.
#[test]
fn the_question_names_the_command_or_file_not_the_category() {
    let (approval, description) = required_approval(&ActionProposal::RunCommand {
        executable: "git".into(),
        args: vec!["push".into(), "origin".into(), "main".into()],
        stdin: None,
    })
    .unwrap();
    assert_eq!(approval, Approval::Publish);
    assert!(description.contains("git push origin main"));

    let (approval, description) = required_approval(&ActionProposal::ReplaceText {
        path: "Cargo.toml".into(),
        expected_hash: "h".into(),
        find: "version = \"0.1.0\"".into(),
        replace: "version = \"9.9.9\"".into(),
    })
    .unwrap();
    assert_eq!(approval, Approval::DependencyChange);
    assert!(description.contains("Cargo.toml"));
    assert!(description.contains("version = \"0.1.0\""));
}

#[test]
fn an_action_needing_nothing_asks_nothing() {
    assert!(
        required_approval(&ActionProposal::ReadFile {
            path: "src/lib.rs".into(),
            first_line: None,
            max_lines: None,
        })
        .is_none()
    );
    assert!(
        required_approval(&ActionProposal::WriteFile {
            path: "src/new.rs".into(),
            content: "x".into(),
        })
        .is_none()
    );
}

/// A long fragment is shortened for the prompt but not hidden.
#[test]
fn a_long_fragment_is_elided_rather_than_dropped() {
    let (_, description) = required_approval(&ActionProposal::ReplaceText {
        path: "package.json".into(),
        expected_hash: "h".into(),
        find: "a".repeat(500),
        replace: "b".into(),
    })
    .unwrap();
    assert!(description.contains("package.json"));
    assert!(description.contains('…'));
    assert!(description.len() < 200);
}

/// Where nobody is watching, refusing is the only safe default: blocking hangs
/// forever and assuming yes removes the boundary.
#[tokio::test]
async fn the_default_prompt_refuses_without_asking() {
    let decision = DenyWithoutAsking
        .ask(Approval::NetworkAccess, "fetch a dependency")
        .await;
    assert_eq!(decision, ApprovalDecision::Deny);
}

#[tokio::test]
async fn a_scripted_grant_is_delivered_to_the_caller() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let prompt = ScriptedPrompt {
        answers: Mutex::new(vec![ApprovalDecision::AllowForRun]),
        asked: asked.clone(),
    };
    let decision = prompt.ask(Approval::Publish, "run `git push`").await;
    assert_eq!(decision, ApprovalDecision::AllowForRun);
    assert_eq!(asked.lock().unwrap()[0].0, Approval::Publish);
}

// ------------------------------------------------- inside the action loop

use poorai_domain::{
    BackendState, ChatMessage, DeploymentDescriptor, ModelChunk, ModelInspection, ModelRequest,
};
use poorai_provider::{ModelProvider, ModelStream, ProviderError};
use poorai_store::Store;
use poorai_tools::{SandboxPolicy, ToolPolicy};
use std::time::Duration;

/// Proposes the same gated edit twice, then completes.
struct TwiceGatedProvider {
    turn: Arc<Mutex<usize>>,
    hashes: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl ModelProvider for TwiceGatedProvider {
    async fn inspect(&self, _: &DeploymentDescriptor) -> Result<ModelInspection, ProviderError> {
        unreachable!()
    }
    async fn runtime_state(&self) -> Result<BackendState, ProviderError> {
        unreachable!()
    }
    async fn chat(&self, _: ModelRequest) -> Result<ModelStream, ProviderError> {
        let mut turn = self.turn.lock().unwrap();
        *turn += 1;
        let hash = self.hashes.lock().unwrap().pop().unwrap_or_default();
        let action = match *turn {
            1 | 2 => serde_json::json!({
                "capability": "replace_text", "path": "Cargo.toml",
                "expected_hash": hash, "find": "0.1.0", "replace": "0.2.0"
            }),
            _ => serde_json::json!({"capability": "complete", "rationale": "done"}),
        };
        Ok(Box::pin(futures_util::stream::iter([Ok(ModelChunk {
            content: action.to_string(),
            done: true,
            ..Default::default()
        })])))
    }
}

fn gated_run(answers: Vec<ApprovalDecision>) -> (Store, poorai_domain::Id, String) {
    let root = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let body = "[package]\nversion = \"0.1.0\"\n";
    std::fs::write(root.path().join("Cargo.toml"), body).unwrap();
    let policy = ToolPolicy {
        root: root.path().to_path_buf(),
        extra_readable: Vec::new(),
        allow_commands: vec![],
        output_limit: 64 * 1024,
        timeout: Duration::from_secs(5),
        sandbox: SandboxPolicy::Disabled,
        approvals: Vec::new(),
    };
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    let hashes = Arc::new(Mutex::new(vec![
        poorai_domain::hash_bytes("[package]\nversion = \"0.2.0\"\n"),
        poorai_domain::hash_bytes(body),
    ]));
    let prompt = ScriptedPrompt {
        answers: Mutex::new(answers),
        asked: Arc::new(Mutex::new(Vec::new())),
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
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "bump".into(),
        }],
        context_tokens: 4096,
        tools: None,
        seed: None,
        sampling: Default::default(),
    };
    let _ = tokio::runtime::Runtime::new().unwrap().block_on(
        poorai_orchestrator::run_action_loop_with_prompt(
            &store,
            &TwiceGatedProvider {
                turn: Arc::new(Mutex::new(0)),
                hashes,
            },
            run_id,
            request,
            &policy,
            &[],
            5,
            &prompt,
            false,
        ),
    );
    let after = std::fs::read_to_string(root.path().join("Cargo.toml")).unwrap();
    (store, run_id, after)
}

fn decisions(store: &Store, run_id: poorai_domain::Id) -> Vec<String> {
    store
        .events_for_run(run_id)
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == "approval.decision")
        .map(|e| {
            e.payload["decision"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A refusal at the prompt is a refusal at the tool: the manifest is untouched.
#[test]
fn a_refused_grant_leaves_the_file_alone_and_is_audited() {
    let (store, run_id, after) = gated_run(vec![ApprovalDecision::Deny, ApprovalDecision::Deny]);
    assert!(after.contains("0.1.0"), "the edit went through anyway");
    assert_eq!(decisions(&store, run_id), vec!["deny", "deny"]);
}

/// A one-time grant expires with the action it was given for, so the second
/// attempt asks again rather than inheriting the first answer.
#[test]
fn a_once_grant_does_not_carry_to_the_next_action() {
    let (store, run_id, after) =
        gated_run(vec![ApprovalDecision::Deny, ApprovalDecision::AllowOnce]);
    assert!(after.contains("0.2.0"), "the granted edit did not apply");
    // Asked twice: the grant did not persist.
    assert_eq!(decisions(&store, run_id), vec!["allow_once", "deny"]);
}

/// A run-wide grant is asked once and covers the rest.
#[test]
fn a_run_grant_is_asked_once() {
    let (store, run_id, _) = gated_run(vec![ApprovalDecision::AllowForRun]);
    assert_eq!(decisions(&store, run_id), vec!["allow_for_run"]);
}

/// A workspace with no checks cannot complete, which is correct and leaves no
/// way forward. The two toolchain-provisioning runs wrote correct programs
/// into workspaces created from nothing and are failures under that rule.
///
/// The way out cannot be the agent running whatever it nominates: a command
/// nobody authorised is not a verifier, and one the agent both chooses and
/// trusts is the agent marking its own work. So the question a person is asked
/// has to name the command, not the category.
#[test]
fn a_proposed_verifier_asks_a_person_and_names_the_command() {
    let (approval, description) = required_approval(&ActionProposal::ProposeVerifier {
        executable: "python3".into(),
        args: vec!["-m".into(), "pytest".into(), "-q".into()],
        rationale: "the workspace has no build system; this runs the tests I wrote".into(),
    })
    .unwrap();
    assert_eq!(approval, Approval::VerifierProposal);
    assert!(
        description.contains("python3 -m pytest -q"),
        "{description}"
    );
    // And why, because a command alone is not enough to judge one by.
    assert!(description.contains("no build system"), "{description}");
}

/// The proposal is refused before anyone is asked when it cannot be judged, or
/// when a whole command line was put where a program name belongs -- the same
/// shape `run_command` refuses, which reached exec as one filename and came
/// back looking like a missing program.
#[test]
fn a_verifier_proposal_that_cannot_be_judged_is_refused() {
    let unjudgeable = ActionProposal::ProposeVerifier {
        executable: "pytest".into(),
        args: vec![],
        rationale: "  ".into(),
    };
    assert!(unjudgeable.validate().is_err());

    let command_line = ActionProposal::ProposeVerifier {
        executable: "python3 -m pytest".into(),
        args: vec![],
        rationale: "runs the tests".into(),
    };
    let error = command_line.validate().unwrap_err().to_string();
    assert!(error.contains("command line"), "{error}");
}

/// Nobody attached means nobody approved. A run that self-approves its own
/// verifier has no boundary at all -- it is the agent deciding what counts as
/// proof of its own work.
#[tokio::test]
async fn nobody_attached_means_no_verifier_is_adopted() {
    let decision = DenyWithoutAsking
        .ask(Approval::VerifierProposal, "adopt `pytest -q`")
        .await;
    assert_eq!(decision, ApprovalDecision::Deny);
}
