# Observability

Emit structured `tracing` events keyed by run, task, model digest, deployment, profile, and repository snapshot. Trace: discovery, selection rationale, provider calls, token estimates/reports, tools, state transitions, verification, error taxonomy, and resource samples.

Metrics include latency histogram, generated tokens/sec, context-tier selections, tool success, policy denials, verification outcomes, recovery count, and calibration stability. Logs must support a replay report without retaining source contents by default. Sampling/redaction rules are configuration and testable. Export local JSONL first; OpenTelemetry is an optional adapter.

## Implementation status — 2026-09-03

**Almost none of the above exists.** `poorai-observe` is seven lines that hash a payload, and no crate in the runtime depends on it. There is no JSONL export, no replay report, no latency histogram, no resource sampling, and no configurable sampling or redaction.

What does exist is the event log, which carries more than this document credits it with: every run appends typed events under one identifier inside a hash chain — provenance, state transitions, every tool attempt allowed or denied, verification results, compaction, plans, named loops, recovery decisions — and since 2026-09-03 each turn records the backend's own counters, prompt and generated tokens and both halves of the duration. `poorai report` reads it back, and an evaluation counts events by type into its outcomes.

**The layer exists as of 2026-09-03.** The events are typed, `report --format jsonl` writes the trail as one record per line, and `replay` folds it into counts: actions by outcome, turns, prompt and generated tokens, backend generation time separately from wall clock, named loops and non-progress, compactions, context downgrades, delivery divergences, turns under memory pressure, and the outcome. Everything is counted from events rather than asserted. A line this build cannot read is skipped and counted; a record whose type it does not know is left out of the export with the count stated.

Memory pressure is sampled after each turn rather than once at admission, so a run that starts on a quiet machine and ends on a saturated one now records the difference.

An export carries the stored hash as an identifier, not as something it can re-verify: the hash covers the event's id, run, payload, timestamp and the link before it, and an export carries only some of those. Whether the chain holds is the store's question, and `report` asks it there.

Still absent: a latency histogram, and a retention policy with configurable sampling and redaction. OpenTelemetry remains an optional adapter nobody has needed.
