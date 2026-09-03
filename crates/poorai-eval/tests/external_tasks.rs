//! Tasks set in a real repository rather than in files written inline.
//!
//! The point of an external corpus is that the ground truth is the project's,
//! not ours: the defect is the one that was really there, and the hidden test
//! is the regression test the upstream fix really added. That only holds if the
//! workspace is pinned, so these check the pinning.

use poorai_eval::{RepositorySource, Suite, Task, TaskKind, Verifier};
use std::collections::BTreeMap;

fn source() -> RepositorySource {
    RepositorySource {
        url: "https://example.invalid/repo.git".into(),
        commit: "1111111111111111111111111111111111111111".into(),
        fix_commit: "2222222222222222222222222222222222222222".into(),
        fix_committed_at: "2026-01-01T00:00:00Z".into(),
        tree_hash: "3333333333333333333333333333333333333333".into(),
        setup: vec![],
    }
}

fn task() -> Task {
    Task {
        id: "t".into(),
        kind: TaskKind::Bugfix,
        statement: "fix it".into(),
        allowed_files: vec!["src/lib.py".into()],
        files: BTreeMap::new(),
        repository: Some(source()),
        visible_verifier: Verifier {
            executable: "python3".into(),
            args: vec![],
        },
        hidden_verifier: Verifier {
            executable: "python3".into(),
            args: vec![],
        },
        hidden_files: BTreeMap::new(),
        expected_in_rationale: None,
        protected_files: vec![],
        max_actions: None,
        approvals: vec![],
        time_budget_secs: 60,
        provenance: "a fixture".into(),
        must_not_happen: None,
    }
}

fn suite_of(tasks: Vec<Task>) -> Result<Suite, String> {
    let suite = Suite {
        name: "s".into(),
        tasks,
    };
    let path = tempfile::tempdir().unwrap().keep().join("s.json");
    std::fs::write(&path, serde_json::to_vec(&suite).unwrap()).unwrap();
    Suite::load(&path).map_err(|e| e.to_string())
}

/// An external task names its files by path in a tree it does not carry, so the
/// "allowed file must be in the workspace" rule cannot apply to it.
#[test]
fn an_external_task_needs_no_inline_files() {
    assert!(suite_of(vec![task()]).is_ok());
}

#[test]
fn a_task_with_neither_files_nor_a_repository_is_refused() {
    let mut task = task();
    task.repository = None;
    let error = suite_of(vec![task]).unwrap_err();
    assert!(error.contains("neither files nor a repository"), "{error}");
}

/// Which of the two is the workspace would be anyone's guess, and a corpus that
/// has to be guessed at is not frozen.
#[test]
fn a_task_declaring_both_is_refused() {
    let mut task = task();
    task.files.insert("src/lib.py".into(), "print(1)\n".into());
    let error = suite_of(vec![task]).unwrap_err();
    assert!(
        error.contains("both inline files and a repository"),
        "{error}"
    );
}

/// A commit id is a content address, so the tree should never differ. It is
/// checked anyway, because a silent mismatch would mean a run was measured
/// against a workspace nobody declared — and that is the one outcome worth
/// ruling out rather than trusting.
#[test]
fn a_checkout_that_does_not_match_the_declared_tree_is_refused() {
    let upstream = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(upstream.path())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@e"]);
    git(&["config", "user.name", "T"]);
    std::fs::write(upstream.path().join("a.txt"), "one\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "one"]);
    // The upstream repository must serve its own commits to a local clone.
    git(&["config", "uploadpack.allowAnySHA1InWant", "true"]);
    let commit = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(upstream.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let mut source = source();
    source.url = upstream.path().display().to_string();
    source.commit = commit;
    // Deliberately wrong.
    source.tree_hash = "0".repeat(40);

    let into = tempfile::tempdir().unwrap();
    let error = poorai_eval::materialise_repository(&source, into.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("but the corpus declares"), "{error}");
}

/// Without a declared tree hash there is nothing to verify the checkout
/// against, so materialising is refused rather than done unchecked.
#[test]
fn a_source_without_a_tree_hash_is_refused() {
    let mut source = source();
    source.tree_hash = String::new();
    let into = tempfile::tempdir().unwrap();
    let error = poorai_eval::materialise_repository(&source, into.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("declares no tree hash"), "{error}");
}
