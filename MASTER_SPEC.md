# poorAI Master Specification

## Purpose

poorAI executes coding tasks against a local repository with open-weight models while making resource use explicit, measurable, and recoverable. The product decision is to optimize the **system**—model, machine, repository, tools, and verification—not to claim a universally best model.

## Normative requirements

1. The production runtime is Rust; Python cannot be required to start, plan, execute, or verify a task.
2. All model access passes through a provider-neutral trait. MVP implementation is Ollama-local.
3. An execution begins with a `RuntimeSnapshot`; it produces an immutable event log and `EvaluationRun` when measured.
4. Context allocation MUST use hardware discovery, model metadata, backend state, and calibration evidence. Fixed “RAM-to-context” formulae are prohibited.
5. Tools use least privilege, declared working roots, time/output limits, and structured audit events.
6. A task is complete only after deterministic verification appropriate to the repository succeeds, or the runtime records a bounded, actionable failure.
7. Routing between models is deferred until comparable calibration/evaluation data exists; MVP exposes explicit selection.

## Core entities

`ModelDefinition` identifies immutable model facts; `ModelStrategy` expresses task policy; `DeploymentDescriptor` describes a reachable serving deployment; `HardwareProfile` describes the machine; `RuntimeSnapshot` captures volatile availability; `CalibrationProfile` records measured operating limits; `ExecutionProfile` is the chosen per-task configuration; `EvaluationRun` is reproducible evidence.

## MVP flow

Discover machine → query Ollama/model → refresh runtime state → select an evidence-bounded profile → index repository → plan → act through tools → verify → recover or report → persist telemetry. All state transitions are evented and resumable only at safe checkpoints.

## Evidence labels

- **Requirement**: binding behaviour in this package.
- **Fact (verify)**: externally observable, versioned datum; record source, command/API response, and date.
- **Heuristic**: provisional policy; measure and replace or tune from results.
- **Hypothesis**: experiment to test; never make it a default without evidence.

Official integration facts must be rechecked against the installed Ollama version and [Ollama API documentation](https://docs.ollama.com/api). Model capabilities and context limits are deployment properties, not names embedded in source.

## Initial laboratory

Primary: `qwen3.8:27b-mlx`. Challenger: `ornith-1.5:35b`. Additional controls: `granite4.2:30b-q6_K`, `nemotron-3.5-lightning:30b-mlx`, `gpt-oss:20b`, `gemma4:31b-mlx`, `muse-glimmer:30b-mlx`. Presence is a **Fact (verify locally)** via `ollama ls`; capability is not assumed from the tag.

See the linked documents for implementation contracts.

## Conformance — 2026-09-03

A requirement is not met by a component that implements it if the production path does not reach that component. Each rule below records where it is actually enforced, following the audit of `cee5ebd`. `docs/roadmap.md` carries the detail and the backlog.

1. **Rust runtime.** Met. No Python is required to start, plan, execute or verify.
2. **Provider-neutral trait.** Met. `chat_cancellable` takes a cancellation handle, and abandoning a reply closes the connection the backend is writing to — asserted by a fixture that observes the close from the server's side, not by the client reporting it.
3. **`RuntimeSnapshot`, immutable log, `EvaluationRun`.** Met. The snapshot preserves backend residency and participates in admission; every run appends to the hash-chained log; an evaluation writes a validated `EvaluationRun` beside its report. The log is append-only through its API but not enforced by the store, and its chain is global rather than per run.
4. **Context allocation from evidence.** Met on the path as of this date. The resolved execution context is what the request carries; no model profile may substitute a static default for it. Calibration still measures what can be allocated rather than what a full context costs, so the evidence is weaker than the rule implies.
5. **Least privilege for tools.** Met for writes and execution: declared roots, derived allowlist, seatbelt confinement, bounded incremental output, process-group kill on timeout, and an audit of every attempt, allowed or denied. Not met for reads — the sandbox does not confine reading outside the workspace — and evaluation setup and external verifiers still execute outside the tool policy entirely.
6. **Completion only after verification.** Met. A repository offering no deterministic check cannot complete: the run records a bounded failure naming the absent verifier rather than reporting success it cannot support.
7. **Explicit model selection.** Met, and now narrowed: a run requires capability evidence matching the deployment's digest and fingerprint, so a tag alone selects nothing.
