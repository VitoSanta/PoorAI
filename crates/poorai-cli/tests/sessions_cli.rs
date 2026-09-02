//! Session listing and inspection, through the binary a user actually runs.
//!
//! A session resumed onto a different branch from the one it was opened on is
//! the case worth seeing before resuming rather than after, so the two states
//! are reported side by side rather than merged into one "current" answer.

use std::process::Command;

fn poorai() -> Command {
    Command::new(env!("CARGO_BIN_EXE_poorai"))
}

fn git(root: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?} failed");
}

/// A workspace holding one session whose only run edited a file on `main`.
fn workspace() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let path = root.path();
    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.email", "t@e"]);
    git(path, &["config", "user.name", "T"]);
    std::fs::write(path.join("code.py"), "x = 1\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "base"]);

    std::fs::create_dir_all(path.join(".poorai")).unwrap();
    let store = poorai_store::Store::open(path.join(".poorai/state.sqlite")).unwrap();
    let run = poorai_domain::new_id();
    store
        .append(
            Some(run),
            "session.opened",
            serde_json::json!({
                "name": "refactor",
                "root": path.display().to_string(),
                "continues_runs": 0,
                "version_control": {"branch": "main", "head": "abc123", "uncommitted_files": 0},
            }),
        )
        .unwrap();
    store
        .append(
            Some(run),
            "run.started",
            serde_json::json!({"task": "rename the helper"}),
        )
        .unwrap();
    store
        .append(
            Some(run),
            "tool.action",
            serde_json::json!({
                "status": "allowed",
                "action": {"capability": "replace_text", "path": "code.py"},
                "outcome": {"new_hash": poorai_domain::hash_bytes("x = 1\n")},
            }),
        )
        .unwrap();
    root
}

fn run_json(root: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let output = poorai()
        .args(args)
        .current_dir(root)
        .output()
        .expect("poorai");
    serde_json::from_slice(&output.stdout).expect("json on stdout")
}

#[test]
fn the_listing_names_the_session_and_what_it_was_last_asked() {
    let root = workspace();
    let value = run_json(root.path(), &["--json", "session", "list"]);
    let sessions = value["result"]["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["name"], "refactor");
    assert_eq!(sessions[0]["runs"], 1);
    // Enough to choose between sessions without opening each one.
    assert_eq!(sessions[0]["last_task"], "rename the helper");
}

#[test]
fn showing_a_session_reports_where_it_opened_and_where_the_workspace_is_now() {
    let root = workspace();
    // The user moves to another branch before resuming.
    git(root.path(), &["checkout", "-q", "-b", "experiment"]);
    let value = run_json(root.path(), &["--json", "session", "show", "refactor"]);
    let result = &value["result"];
    assert_eq!(result["opened_on"]["branch"], "main");
    assert_eq!(
        result["workspace_now"]["branch"], "experiment",
        "the branch the workspace is on now was not reported"
    );
    assert!(result["ledger"].as_str().unwrap().contains("code.py"));
}

#[test]
fn an_unknown_session_is_refused_rather_than_answered_empty() {
    let root = workspace();
    let value = run_json(root.path(), &["--json", "session", "show", "no-such"]);
    assert_eq!(value["ok"], false);
    assert!(
        value["error"]["context"]
            .as_str()
            .unwrap()
            .contains("no session named")
    );
}

/// A workspace that is not a git checkout has no branch, and saying so is
/// honest where inventing `main` is not.
#[test]
fn a_workspace_outside_version_control_reports_no_branch() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".poorai")).unwrap();
    let store = poorai_store::Store::open(root.path().join(".poorai/state.sqlite")).unwrap();
    let run = poorai_domain::new_id();
    store
        .append(
            Some(run),
            "session.opened",
            serde_json::json!({"name": "plain", "root": "/w", "version_control": {}}),
        )
        .unwrap();
    store
        .append(Some(run), "run.started", serde_json::json!({"task": "t"}))
        .unwrap();
    let value = run_json(root.path(), &["--json", "session", "show", "plain"]);
    assert!(value["result"]["workspace_now"].get("branch").is_none());
}
