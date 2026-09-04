//! A run's state, rebuilt from its own log.
//!
//! Sessions carried facts forward; nothing resumed. `TaskCheckpoint` was
//! persisted on every transition and never read back, so "all state
//! transitions are evented and resumable" described half a mechanism.

use poorai_domain::{RunEvent, TaskCheckpointRecord, ToolActionStatus, new_id, now};
use poorai_orchestrator::{RunState, RunTerminal, TaskState};

fn transition(state: &str) -> RunEvent {
    RunEvent::TaskTransition(TaskCheckpointRecord {
        id: new_id(),
        state: state.into(),
        detail: "fixture".into(),
        at: now(),
    })
}

fn edit(path: &str, hash: &str) -> RunEvent {
    RunEvent::ToolAction {
        action: serde_json::json!({"capability": "apply_replace", "path": path}),
        status: ToolActionStatus::Allowed,
        outcome_class: "allowed_success".into(),
        outcome: Some(serde_json::json!({"path": path, "new_hash": hash})),
        denial: None,
        failure: None,
        failure_category: None,
    }
}

fn denied() -> RunEvent {
    RunEvent::ToolAction {
        action: serde_json::json!({"capability": "apply_replace", "path": "a.rs"}),
        status: ToolActionStatus::Denied,
        outcome_class: "policy_denial".into(),
        outcome: None,
        denial: Some("stale hash".into()),
        failure: None,
        failure_category: None,
    }
}

#[test]
fn the_state_is_the_fold_of_what_was_recorded() {
    let events = vec![
        transition("Act"),
        RunEvent::TaskPlan {
            steps: vec!["read".into(), "fix".into()],
            note: None,
        },
        edit("a.rs", "hash-a"),
        edit("a.rs", "hash-a2"),
        edit("b.rs", "hash-b"),
        transition("Verify"),
    ];
    let state = RunState::replay(&events);
    assert_eq!(state.state, Some(TaskState::Verify));
    assert_eq!(state.actions_spent, 3);
    // The latest hash of each file, not every hash it ever had: an edit
    // guarded by a stale one is the loop this project already closed once.
    assert_eq!(state.changed_files["a.rs"], "hash-a2");
    assert_eq!(state.changed_files["b.rs"], "hash-b");
    assert_eq!(state.plan.len(), 2);
    assert!(state.terminal.is_none());
}

/// A denied attempt performed nothing and cost no action. The replay has to
/// apply the same rule as the live loop or a resumed run starts with a budget
/// that does not match the one it was spending.
#[test]
fn a_denial_costs_no_action_on_replay_either() {
    let state = RunState::replay(&[transition("Act"), denied(), edit("a.rs", "h")]);
    assert_eq!(state.actions_spent, 1);
}

/// A verifier a person approved outlives the run it was approved in. Asking
/// again on resume would be asking someone to authorise the same command twice
/// for the same work.
#[test]
fn an_adopted_verifier_survives_into_the_replayed_state() {
    let state = RunState::replay(&[
        transition("Act"),
        RunEvent::VerifierAdopted {
            step: 1,
            executable: "pytest".into(),
            args: vec!["-q".into()],
            source: "approved".into(),
        },
        RunEvent::VerifierAdopted {
            step: 2,
            executable: "pytest".into(),
            args: vec!["-q".into()],
            source: "approved".into(),
        },
    ]);
    assert_eq!(
        state.adopted_verifiers,
        vec![("pytest".to_string(), vec!["-q".to_string()])],
        "the same verifier adopted twice is one verifier"
    );
}

/// The context a resumed run should send is the one the original had stepped
/// down to, not the one it started with. Resuming at the higher tier would
/// reproduce the failure that caused the downgrade.
#[test]
fn the_replayed_context_is_the_one_after_any_downgrade() {
    let state = RunState::replay(&[
        transition("Act"),
        RunEvent::ContextTierChanged {
            previous_context_tokens: 8192,
            context_tokens: 4096,
            evidence: "measured".into(),
            provider_error: "context".into(),
            attempt: 1,
        },
        RunEvent::ContextTierChanged {
            previous_context_tokens: 4096,
            context_tokens: 2048,
            evidence: "measured".into(),
            provider_error: "context".into(),
            attempt: 2,
        },
    ]);
    assert_eq!(state.context_tokens, Some(2048));
}

/// A run that ended is reported, not resumed. Every way out writes a terminal
/// event -- including an interruption, which the drop guard records -- so the
/// absence of one is the signature of a crash, a kill, or a machine going
/// away.
#[test]
fn only_a_run_with_no_terminal_event_is_resumable() {
    let running = RunState::replay(&[transition("Act"), edit("a.rs", "h")]);
    assert!(running.interrupted());

    let completed = RunState::replay(&[
        transition("Act"),
        RunEvent::TaskComplete {
            step: 1,
            verified: true,
        },
    ]);
    assert!(!completed.interrupted());
    assert_eq!(
        completed.terminal,
        Some(RunTerminal::Complete { verified: true })
    );

    let failed = RunState::replay(&[
        transition("Act"),
        RunEvent::TaskFailed {
            reason: "no verifier".into(),
            class: poorai_domain::TerminalClass::NoVerifier,
            detail: None,
        },
    ]);
    assert!(!failed.interrupted());

    // A log with no transitions at all is not an interrupted run; it is a run
    // that never started, and there is nothing to resume into.
    assert!(!RunState::replay(&[]).interrupted());
}

/// Through SQLite, which is the path that matters: a crash means the process
/// is gone and the only thing left is what the database holds. Replaying from
/// in-memory values proves the fold; this proves the round trip.
#[test]
fn an_interrupted_run_is_recovered_from_the_database() {
    let store = poorai_store::Store::open(":memory:").unwrap();
    let run_id = new_id();
    for event in [
        transition("Act"),
        RunEvent::TaskPlan {
            steps: vec!["fix the parser".into()],
            note: None,
        },
        edit("parser.rs", "hash-1"),
        RunEvent::VerifierAdopted {
            step: 1,
            executable: "pytest".into(),
            args: vec!["-q".into()],
            source: "approved".into(),
        },
        // and then the machine goes away: no terminal event.
    ] {
        store.append_event(Some(run_id), &event).unwrap();
    }
    // A second run interleaved in the same database must not appear in this
    // run's state -- the chain is global and the events are not.
    let other = new_id();
    store
        .append_event(Some(other), &edit("elsewhere.rs", "hash-x"))
        .unwrap();

    let state = RunState::replay(&store.typed_events_for_run(run_id).unwrap());
    assert!(state.interrupted(), "the crash was not visible in the log");
    assert_eq!(state.actions_spent, 1);
    assert_eq!(state.changed_files["parser.rs"], "hash-1");
    assert!(!state.changed_files.contains_key("elsewhere.rs"));
    assert_eq!(state.plan, vec!["fix the parser".to_string()]);
    assert_eq!(state.adopted_verifiers.len(), 1);
}

/// The guard I wrote against unread configuration checked declared profiles,
/// not the loop's own tuning -- and `turn_timeout` was silently unread for
/// several commits after a bulk edit clobbered its wiring. A field that no
/// production path reads is the defect this whole audit was about, and it does
/// not stop being one because I introduced it.
#[test]
fn every_tuning_field_reaches_the_loop() {
    let source = include_str!("../src/lib.rs");
    for (field, evidence) in [
        ("malformed_call_limit", "tuning.malformed_call_limit"),
        ("turn_timeout", "tuning.turn_timeout"),
        ("host", "tuning.host"),
        ("full_checks", "tuning.full_checks"),
    ] {
        assert!(
            source.contains(evidence),
            "RunTuning::{field} is declared and never read"
        );
    }
}
