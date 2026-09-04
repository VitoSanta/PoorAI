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
        extra_readable: Vec::new(),
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
            ..Default::default()
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
        extra_readable: Vec::new(),
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

/// Repetition of a *refused* action was the only non-progress the loop could
/// see. A run of successful reads in a circle spends the budget just as
/// completely, over a repository that stays exactly where it was.
mod no_progress {
    use super::*;

    /// Answers from a script, cycling once it runs out.
    struct ScriptedProvider {
        script: Vec<String>,
        next: Arc<Mutex<usize>>,
    }
    #[async_trait]
    impl ModelProvider for ScriptedProvider {
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
            let mut next = self.next.lock().unwrap();
            let content = self.script[*next % self.script.len()].clone();
            *next += 1;
            Ok(Box::pin(futures_util::stream::iter([Ok(ModelChunk {
                content,
                done: true,
                ..Default::default()
            })])))
        }
    }

    fn run_script(files: &[(&str, &str)], script: Vec<String>, max_actions: u8) -> usize {
        let root = tempfile::tempdir().unwrap();
        for (name, body) in files {
            std::fs::write(root.path().join(name), body).unwrap();
        }
        let policy = ToolPolicy {
            root: root.path().to_path_buf(),
            extra_readable: Vec::new(),
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
                ..Default::default()
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
                    &ScriptedProvider {
                        script,
                        next: Arc::new(Mutex::new(0)),
                    },
                    run_id,
                    request,
                    &policy,
                    &[],
                    max_actions,
                ));
        store
            .events_for_run(run_id)
            .unwrap()
            .iter()
            .filter(|e| e.event_type == "no_progress.detected")
            .count()
    }

    fn read(path: &str) -> String {
        format!(r#"{{"capability":"read_file","path":"{path}"}}"#)
    }

    #[test]
    fn reading_the_same_files_in_a_circle_is_named() {
        let detections = run_script(
            &[("a.rs", "one"), ("b.rs", "two")],
            vec![read("a.rs"), read("b.rs")],
            14,
        );
        assert!(detections > 0, "a circle of successful reads went unnamed");
    }

    /// The guard that matters. Reading files it has never read is exactly what
    /// a deployment should do on a task it does not yet understand, and
    /// interrupting that would be worse than the problem.
    #[test]
    fn reading_files_it_has_not_read_before_is_not_named() {
        let files: Vec<(String, String)> = (0..8)
            .map(|i| (format!("f{i}.rs"), format!("body {i}")))
            .collect();
        let borrowed: Vec<(&str, &str)> = files
            .iter()
            .map(|(name, body)| (name.as_str(), body.as_str()))
            .collect();
        let script: Vec<String> = (0..8).map(|i| read(&format!("f{i}.rs"))).collect();
        assert_eq!(
            run_script(&borrowed, script, 8),
            0,
            "investigation was reported as going nowhere"
        );
    }

    /// An edit and its revert leave the workspace where it started, having
    /// spent two actions and changed the signature twice on the way.
    #[test]
    fn an_edit_and_its_revert_is_named() {
        let script = vec![
            r#"{"capability":"write_file","path":"new.rs","content":"x"}"#.into(),
            r#"{"capability":"read_file","path":"code.rs"}"#.into(),
        ];
        // write_file refuses to overwrite, so after the first pass every later
        // action leaves the workspace exactly as it was.
        let detections = run_script(&[("code.rs", "one")], script, 14);
        assert!(detections > 0, "a run changing nothing went unnamed");
    }
}

/// Naming non-progress and continuing was the original choice. A real
/// generation task showed what it costs: two hundred actions, a hundred and
/// ten reads, `npm run build` seventeen times, and not one write. The loop
/// said so eleven times and the deployment read on.
#[test]
fn repeated_windows_of_nothing_end_the_run() {
    let source = include_str!("../src/lib.rs");
    assert!(
        source.contains("NO_PROGRESS_LIMIT"),
        "non-progress is named and never bounded"
    );
    let arm = source
        .split("if no_progress_windows >= NO_PROGRESS_LIMIT")
        .nth(1)
        .expect("the bound does not end the run");
    let arm = &arm[..arm.len().min(900)];
    assert!(
        arm.contains("persist_failure"),
        "the ending is not recorded"
    );
    assert!(arm.contains("return Err"), "the run continues anyway");
}

/// The bound has to be looser than the window, or a single quiet stretch --
/// six reads while working out what to change -- ends a run that was fine.
#[test]
fn one_quiet_window_does_not_end_a_run() {
    let source = include_str!("../src/lib.rs");
    let limit: usize = source
        .split("const NO_PROGRESS_LIMIT: usize = ")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .and_then(|value| value.trim().parse().ok())
        .expect("no limit declared");
    assert!(limit >= 2, "a single window ends the run");
}
