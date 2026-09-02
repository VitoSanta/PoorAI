//! Resuming a named session.
//!
//! The facts an earlier run recorded were true when it recorded them. Between
//! runs a file can be edited by hand, by a colleague, or by a merge, so a
//! ledger that replays a recorded hash can hand the next run a hash the
//! workspace no longer has -- which is the stale-hash loop this project spent a
//! campaign removing, reintroduced through the back door.

use poorai_orchestrator::session_ledger;
use poorai_store::Store;

/// Records the audit of a run that read one file and edited another.
fn recorded_run(store: &Store, root: &std::path::Path) -> poorai_domain::Id {
    let run_id = poorai_domain::new_id();
    store
        .append(
            Some(run_id),
            "run.started",
            serde_json::json!({"task": "fix the parser"}),
        )
        .unwrap();
    let edited = std::fs::read(root.join("edited.rs")).unwrap();
    let read = std::fs::read(root.join("read.rs")).unwrap();
    store
        .append(
            Some(run_id),
            "tool.action",
            serde_json::json!({
                "status": "allowed",
                "action": {"capability": "replace_text", "path": "edited.rs"},
                "outcome": {"new_hash": poorai_domain::hash_bytes(&edited)},
            }),
        )
        .unwrap();
    store
        .append(
            Some(run_id),
            "tool.action",
            serde_json::json!({
                "status": "allowed",
                "action": {"capability": "read_file", "path": "read.rs"},
                "outcome": {"artifact_hash": poorai_domain::hash_bytes(&read)},
            }),
        )
        .unwrap();
    run_id
}

fn workspace() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("edited.rs"), "fixed body").unwrap();
    std::fs::write(root.path().join("read.rs"), "read body").unwrap();
    root
}

#[test]
fn a_resumed_session_reports_what_the_earlier_run_changed() {
    let root = workspace();
    let store = Store::open(":memory:").unwrap();
    let run = recorded_run(&store, root.path());
    let ledger = session_ledger(&store, &[run], root.path()).unwrap();
    assert!(ledger.contains("edited.rs"), "{ledger}");
    assert!(ledger.contains("fix the parser"), "{ledger}");
    // The hash offered is the one the next call must pass.
    assert!(
        ledger.contains(&poorai_domain::hash_bytes("fixed body")),
        "{ledger}"
    );
}

/// The case that matters: the workspace moved on between runs.
#[test]
fn a_file_changed_outside_poorai_is_reported_with_its_current_hash() {
    let root = workspace();
    let store = Store::open(":memory:").unwrap();
    let run = recorded_run(&store, root.path());
    // Somebody edits the file after the run ended.
    std::fs::write(root.path().join("edited.rs"), "edited by hand").unwrap();
    let ledger = session_ledger(&store, &[run], root.path()).unwrap();

    let recorded = poorai_domain::hash_bytes("fixed body");
    let current = poorai_domain::hash_bytes("edited by hand");
    assert!(
        !ledger.contains(&recorded),
        "the ledger replayed a hash the workspace no longer has:\n{ledger}"
    );
    assert!(ledger.contains(&current), "{ledger}");
    assert!(ledger.contains("changed outside poorAI"), "{ledger}");
}

#[test]
fn a_file_deleted_between_runs_is_reported_as_gone() {
    let root = workspace();
    let store = Store::open(":memory:").unwrap();
    let run = recorded_run(&store, root.path());
    std::fs::remove_file(root.path().join("edited.rs")).unwrap();
    let ledger = session_ledger(&store, &[run], root.path()).unwrap();
    assert!(ledger.contains("no longer exist"), "{ledger}");
    assert!(
        !ledger.contains(&poorai_domain::hash_bytes("fixed body")),
        "{ledger}"
    );
}

/// Later runs supersede earlier ones rather than accumulating beside them.
#[test]
fn the_last_state_of_a_file_is_the_one_carried() {
    let root = workspace();
    let store = Store::open(":memory:").unwrap();
    let first = recorded_run(&store, root.path());
    std::fs::write(root.path().join("edited.rs"), "second pass").unwrap();
    let second = recorded_run(&store, root.path());
    let ledger = session_ledger(&store, &[first, second], root.path()).unwrap();
    assert_eq!(
        ledger.matches("edited.rs").count(),
        1,
        "the file is listed more than once:\n{ledger}"
    );
    assert!(
        ledger.contains(&poorai_domain::hash_bytes("second pass")),
        "{ledger}"
    );
}

/// `run.started` writes the statement under `task` and the evaluation harness
/// under `request`. A ledger that reads only one silently loses the goal of
/// every session opened by the other.
#[test]
fn the_statement_is_carried_under_either_key_the_audit_uses() {
    for key in ["task", "request"] {
        let root = workspace();
        let store = Store::open(":memory:").unwrap();
        let run = poorai_domain::new_id();
        store
            .append(
                Some(run),
                "run.started",
                serde_json::json!({key: "fix the parser"}),
            )
            .unwrap();
        let ledger = session_ledger(&store, &[run], root.path()).unwrap();
        assert!(ledger.contains("fix the parser"), "{key}: {ledger}");
    }
}

#[test]
fn sessions_are_reconstructed_from_the_event_log() {
    let store = Store::open(":memory:").unwrap();
    let first = poorai_domain::new_id();
    let second = poorai_domain::new_id();
    for (run, name) in [(first, "refactor-auth"), (second, "refactor-auth")] {
        store
            .append(
                Some(run),
                "session.opened",
                serde_json::json!({"name": name, "root": "/w"}),
            )
            .unwrap();
    }
    store
        .append(
            Some(poorai_domain::new_id()),
            "session.opened",
            serde_json::json!({"name": "other", "root": "/w"}),
        )
        .unwrap();
    let sessions = store.sessions().unwrap();
    assert_eq!(sessions.len(), 2);
    let auth = sessions.iter().find(|s| s.name == "refactor-auth").unwrap();
    assert_eq!(auth.runs, vec![first, second]);
    assert_eq!(
        store.session_runs("refactor-auth").unwrap(),
        vec![first, second]
    );
    // An unknown name is empty rather than an error: it opens a new session.
    assert!(store.session_runs("never-used").unwrap().is_empty());
}
