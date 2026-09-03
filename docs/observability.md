# Observability

Emit structured `tracing` events keyed by run, task, model digest, deployment, profile, and repository snapshot. Trace: discovery, selection rationale, provider calls, token estimates/reports, tools, state transitions, verification, error taxonomy, and resource samples.

Metrics include latency histogram, generated tokens/sec, context-tier selections, tool success, policy denials, verification outcomes, recovery count, and calibration stability. Logs must support a replay report without retaining source contents by default. Sampling/redaction rules are configuration and testable. Export local JSONL first; OpenTelemetry is an optional adapter.

## Implementation status — 2026-09-03

**Almost none of the above exists.** `poorai-observe` is seven lines that hash a payload, and no crate in the runtime depends on it. There is no JSONL export, no replay report, no latency histogram, no resource sampling, and no configurable sampling or redaction.

What does exist is the event log, which carries more than this document credits it with: every run appends typed events under one identifier inside a hash chain — provenance, state transitions, every tool attempt allowed or denied, verification results, compaction, plans, named loops, recovery decisions — and since 2026-09-03 each turn records the backend's own counters, prompt and generated tokens and both halves of the duration. `poorai report` reads it back, and an evaluation counts events by type into its outcomes.

So the facts are being captured and the observability layer is missing: what is absent is export, replay, resource sampling over time, and retention policy. Building it means giving the log typed event structs rather than free-form JSON payloads, which is the same prerequisite that a per-run hash chain and an artifact table need.
