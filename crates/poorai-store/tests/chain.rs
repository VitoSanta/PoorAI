//! The chain has to be evidence, not decoration.
//!
//! The API only ever appended, and SQLite permits `UPDATE` and `DELETE`
//! regardless -- so "append-only" was a property of the code rather than of the
//! data, and nothing could tell the difference after the fact.

use poorai_domain::{RunEvent, new_id};
use poorai_store::Store;

fn event(reason: &str) -> RunEvent {
    RunEvent::TaskFailed {
        reason: reason.into(),
        detail: None,
    }
}

#[test]
fn a_runs_chain_holds_and_is_independent_of_other_runs() {
    let store = Store::open(":memory:").unwrap();
    let first = new_id();
    let second = new_id();
    // Interleaved, which is the case the global chain could not separate: a
    // run's events depended on whatever else the database held.
    store.append_event(Some(first), &event("a")).unwrap();
    store.append_event(Some(second), &event("x")).unwrap();
    store.append_event(Some(first), &event("b")).unwrap();
    store.append_event(Some(second), &event("y")).unwrap();
    store.append_event(Some(first), &event("c")).unwrap();

    let verdict = store.verify_run_chain(first).unwrap();
    assert_eq!(verdict.events, 3, "another run's events were counted");
    assert_eq!(
        verdict.unlinked, 1,
        "the first event of a run links to nothing"
    );
    assert!(verdict.intact(), "{verdict:?}");
    assert!(store.verify_run_chain(second).unwrap().intact());
}

/// The point of the whole thing. A payload edited in place still parses, still
/// has a row, and no longer hashes to what was recorded.
#[test]
fn an_edited_payload_breaks_the_chain() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let run_id = new_id();
    {
        let store = Store::open(&path).unwrap();
        for reason in ["a", "b", "c"] {
            store.append_event(Some(run_id), &event(reason)).unwrap();
        }
        assert!(store.verify_run_chain(run_id).unwrap().intact());
    }
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE events SET payload = ?1 WHERE payload LIKE '%\"b\"%'",
            rusqlite::params![r#"{"reason":"something else entirely"}"#],
        )
        .unwrap();
    drop(connection);

    let store = Store::open(&path).unwrap();
    let verdict = store.verify_run_chain(run_id).unwrap();
    assert!(!verdict.intact(), "an edited payload passed verification");
    assert_eq!(verdict.broken_at, Some(2), "{verdict:?}");
}

/// A deleted row leaves the events after it linking to something no longer
/// there.
#[test]
fn a_deleted_event_breaks_the_chain() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let run_id = new_id();
    {
        let store = Store::open(&path).unwrap();
        for reason in ["a", "b", "c"] {
            store.append_event(Some(run_id), &event(reason)).unwrap();
        }
    }
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute("DELETE FROM events WHERE payload LIKE '%\"b\"%'", [])
        .unwrap();
    drop(connection);

    let verdict = Store::open(&path)
        .unwrap()
        .verify_run_chain(run_id)
        .unwrap();
    assert!(!verdict.intact(), "a deleted event passed verification");
}

/// "Verified" must never quietly mean "there was nothing to check".
#[test]
fn an_absence_of_evidence_is_not_an_intact_chain() {
    let store = Store::open(":memory:").unwrap();
    let verdict = store.verify_run_chain(new_id()).unwrap();
    assert_eq!(verdict.events, 0);
    assert!(!verdict.intact());
}
