//! Exporting a run's log, and reading it back into a report.
//!
//! This crate was seven lines that hashed a payload, and no crate in the
//! runtime depended on it. Meanwhile the event log carried more than
//! `observability.md` credited it with: typed events under one identifier,
//! inside a hash chain, with the backend's own counters per turn.
//!
//! So the gap was never capture. It was export, replay, and a report a person
//! can read without a database — which is what this crate is now.

use poorai_domain::{RunEvent, hash_bytes};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;

/// One exported line: the event, and enough around it to order and locate it.
///
/// JSONL because a run's trail is a stream of records, not a document: it can
/// be appended to, tailed, cut with `grep`, and read by something that does not
/// know this schema. That is the format `observability.md` asked for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedEvent {
    pub run_id: String,
    pub sequence: usize,
    pub at: chrono::DateTime<chrono::Utc>,
    pub event_type: String,
    /// The hash the store recorded for this event.
    ///
    /// An identifier, not something this file can re-verify: the stored hash
    /// covers the event's id, run, payload, timestamp and the link before it,
    /// and an export carries only some of those. Whether the chain holds is a
    /// question for the store, which has all of them, and `report` asks it
    /// there. Carrying the hash here is what lets an exported line be matched
    /// back to the row it came from.
    pub event_hash: String,
    pub event: RunEvent,
}

/// Writes a run's events as JSONL.
///
/// Source contents are not retained by default -- the events carry hashes and
/// bounded excerpts, and this writes what they carry rather than reaching back
/// into the workspace for more.
pub fn export_jsonl<W: Write>(events: &[ExportedEvent], out: &mut W) -> std::io::Result<usize> {
    let mut written = 0;
    for event in events {
        let line = serde_json::to_string(event)?;
        writeln!(out, "{line}")?;
        written += 1;
    }
    Ok(written)
}

/// Reads back what `export_jsonl` wrote.
///
/// A line this build cannot parse is skipped and counted rather than failing
/// the whole replay: a trail with one unreadable record is still evidence
/// about the rest, and a reader that refuses everything on one bad line is a
/// reader nobody uses.
pub fn read_jsonl(text: &str) -> (Vec<ExportedEvent>, usize) {
    let mut events = Vec::new();
    let mut unreadable = 0;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str::<ExportedEvent>(line) {
            Ok(event) => events.push(event),
            Err(_) => unreadable += 1,
        }
    }
    (events, unreadable)
}

/// What a run did, computed from its exported trail.
///
/// Every field is counted from events rather than asserted: a replay that
/// summarises what it was told happened is a second account that can disagree
/// with the first.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Replay {
    pub run_id: String,
    pub events: usize,
    pub by_type: BTreeMap<String, usize>,
    pub actions_allowed: usize,
    pub actions_denied: usize,
    pub actions_failed: usize,
    pub turns: usize,
    pub prompt_tokens: u64,
    pub generated_tokens: u64,
    /// Wall clock between the first and last event, in seconds.
    pub elapsed_secs: f64,
    /// Backend-reported generation time, so the share spent waiting on the
    /// model can be told from the share spent running tools.
    pub generation_secs: f64,
    pub loops_named: usize,
    pub no_progress_named: usize,
    pub compactions: usize,
    pub context_downgrades: usize,
    pub delivery_divergences: usize,
    /// Turns that ended with the host observably under memory pressure.
    ///
    /// Counted rather than averaged: pressure is a state a run was in for some
    /// of its turns, and a mean over a run that was fine for forty turns and
    /// saturated for four describes neither.
    pub turns_under_pressure: usize,
    pub outcome: Option<String>,
}

/// Folds an exported trail into a report.
pub fn replay(events: &[ExportedEvent]) -> Replay {
    let mut report = Replay {
        run_id: events.first().map(|e| e.run_id.clone()).unwrap_or_default(),
        events: events.len(),
        ..Default::default()
    };
    for exported in events {
        *report
            .by_type
            .entry(exported.event_type.clone())
            .or_default() += 1;
        match &exported.event {
            RunEvent::ToolAction { status, .. } => match status {
                poorai_domain::ToolActionStatus::Allowed => report.actions_allowed += 1,
                poorai_domain::ToolActionStatus::Denied => report.actions_denied += 1,
                poorai_domain::ToolActionStatus::Failed => report.actions_failed += 1,
            },
            RunEvent::TurnGenerated { metrics, .. } => {
                report.turns += 1;
                if let Some(metrics) = metrics {
                    report.prompt_tokens += metrics.prompt_tokens.unwrap_or(0);
                    report.generated_tokens += metrics.generated_tokens.unwrap_or(0);
                    report.generation_secs += metrics
                        .generation_duration_ns
                        .map(|ns| ns as f64 / 1e9)
                        .unwrap_or(0.0);
                }
            }
            RunEvent::ResourceSampled { pressure, .. } => {
                if matches!(
                    pressure,
                    poorai_domain::Observation::Observed(value)
                        if value.get("under_pressure").and_then(serde_json::Value::as_bool)
                            == Some(true)
                ) {
                    report.turns_under_pressure += 1;
                }
            }
            RunEvent::LoopDetected { .. } => report.loops_named += 1,
            RunEvent::NoProgressDetected { .. } => report.no_progress_named += 1,
            RunEvent::ContextCompacted(_) => report.compactions += 1,
            RunEvent::ContextTierChanged { .. } => report.context_downgrades += 1,
            RunEvent::ContextDeliveryDiverged { .. } => report.delivery_divergences += 1,
            RunEvent::TaskComplete { verified, .. } => {
                report.outcome = Some(if *verified {
                    "complete".into()
                } else {
                    "complete_unverified".into()
                });
            }
            RunEvent::TaskFailed { reason, .. } => {
                report.outcome = Some(format!("failed: {reason}"));
            }
            _ => {}
        }
    }
    if let (Some(first), Some(last)) = (events.first(), events.last()) {
        report.elapsed_secs = (last.at - first.at).num_milliseconds() as f64 / 1000.0;
    }
    report
}

/// Emits a structured trace line, hashing the payload rather than logging it.
///
/// Kept from the crate's earlier life: a trace that carries source contents is
/// a retention decision nobody made.
pub fn emit<T: Serialize>(event_type: &str, payload: &T) {
    let value = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
    tracing::info!(event_type, payload_hash = %hash_bytes(serde_json::to_vec(&value).unwrap_or_default()), "poorai_event");
}
