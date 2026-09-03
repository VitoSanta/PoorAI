//! Export, read back, and report -- without a database.

use poorai_domain::{GenerationMetrics, RunEvent, ToolActionStatus, now};
use poorai_observe::{ExportedEvent, export_jsonl, read_jsonl, replay};

fn exported(sequence: usize, event: RunEvent) -> ExportedEvent {
    ExportedEvent {
        run_id: "run-1".into(),
        sequence,
        at: now(),
        event_type: event.event_type().into(),
        event_hash: format!("hash-{sequence}"),
        event,
    }
}

fn action(status: ToolActionStatus) -> RunEvent {
    RunEvent::ToolAction {
        action: serde_json::json!({"capability": "read_file"}),
        status,
        outcome_class: "allowed_success".into(),
        outcome: None,
        denial: None,
        failure: None,
        failure_category: None,
    }
}

fn trail() -> Vec<ExportedEvent> {
    vec![
        exported(1, RunEvent::RunStarted(serde_json::json!({"task": "fix"}))),
        exported(
            2,
            RunEvent::TurnGenerated {
                step: 0,
                turn: 1,
                metrics: Some(GenerationMetrics {
                    prompt_tokens: Some(1_200),
                    generated_tokens: Some(90),
                    generation_duration_ns: Some(3_000_000_000),
                    ..Default::default()
                }),
                tokens_per_second: Some(30.0),
                thinking_chars: 0,
                content_chars: 40,
                prompt_delivery: None,
            },
        ),
        exported(3, action(ToolActionStatus::Allowed)),
        exported(4, action(ToolActionStatus::Denied)),
        exported(5, action(ToolActionStatus::Failed)),
        exported(
            6,
            RunEvent::NoProgressDetected {
                step: 3,
                window: 6,
                actions: vec!["read_file:a".into()],
            },
        ),
        exported(
            7,
            RunEvent::TaskComplete {
                step: 3,
                verified: true,
            },
        ),
    ]
}

#[test]
fn a_trail_survives_the_round_trip_through_jsonl() {
    let mut out = Vec::new();
    let written = export_jsonl(&trail(), &mut out).unwrap();
    assert_eq!(written, 7);
    let text = String::from_utf8(out).unwrap();
    // One record per line, which is what makes it tailable and greppable.
    assert_eq!(text.lines().count(), 7);

    let (read, unreadable) = read_jsonl(&text);
    assert_eq!(unreadable, 0);
    assert_eq!(read.len(), 7);
    assert_eq!(read[2].event_type, "tool.action");
}

/// A trail with one unreadable record is still evidence about the rest, and a
/// reader that refuses everything on one bad line is a reader nobody uses.
#[test]
fn one_unreadable_line_does_not_discard_the_others() {
    let mut out = Vec::new();
    export_jsonl(&trail(), &mut out).unwrap();
    let mut text = String::from_utf8(out).unwrap();
    text.push_str("{not json at all\n");
    let (read, unreadable) = read_jsonl(&text);
    assert_eq!(read.len(), 7);
    assert_eq!(unreadable, 1, "the bad line was passed over silently");
}

/// Every field is counted from events rather than asserted: a replay that
/// summarises what it was told happened is a second account that can disagree
/// with the first.
#[test]
fn the_report_is_counted_from_the_events() {
    let report = replay(&trail());
    assert_eq!(report.events, 7);
    assert_eq!(report.actions_allowed, 1);
    assert_eq!(report.actions_denied, 1);
    assert_eq!(report.actions_failed, 1);
    assert_eq!(report.turns, 1);
    assert_eq!(report.prompt_tokens, 1_200);
    assert_eq!(report.generated_tokens, 90);
    // The share spent waiting on the model, distinct from wall clock.
    assert_eq!(report.generation_secs, 3.0);
    assert_eq!(report.no_progress_named, 1);
    assert_eq!(report.outcome.as_deref(), Some("complete"));
    assert_eq!(report.by_type["tool.action"], 3);
}

#[test]
fn an_empty_trail_reports_nothing_rather_than_a_success() {
    let report = replay(&[]);
    assert_eq!(report.events, 0);
    assert!(report.outcome.is_none());
}
