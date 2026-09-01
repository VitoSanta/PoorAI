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
