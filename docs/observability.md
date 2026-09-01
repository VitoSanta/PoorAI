# Observability

Emit structured `tracing` events keyed by run, task, model digest, deployment, profile, and repository snapshot. Trace: discovery, selection rationale, provider calls, token estimates/reports, tools, state transitions, verification, error taxonomy, and resource samples.

Metrics include latency histogram, generated tokens/sec, context-tier selections, tool success, policy denials, verification outcomes, recovery count, and calibration stability. Logs must support a replay report without retaining source contents by default. Sampling/redaction rules are configuration and testable. Export local JSONL first; OpenTelemetry is an optional adapter.
