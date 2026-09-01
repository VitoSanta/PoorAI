# poorAI — Local Coding Agent

poorAI is a Rust runtime for reliable software-engineering tasks with local open-weight models. It is **model-aware** (it knows each model's capabilities and limits) and **hardware-aware** (it chooses an execution profile from measured machine and backend state). The MVP serves Ollama locally through a provider-neutral boundary.

## Contract

poorAI does not infer a context window from RAM alone. It discovers hardware, reads model metadata and backend state, then uses an empirical `CalibrationProfile` to select a safe `ExecutionProfile`. Every performance or reliability claim must be backed by a reproducible `EvaluationRun`.

## Repository layout

- `docs/`: normative design, implementation contracts, and ADRs.
- `prompt-codex.md`: bootstrap prompt for implementation in a new Rust repository.
- `MASTER_SPEC.md`: concise, end-to-end specification.

## MVP boundary

Single local workspace; Ollama; one active coding task; safe read/write/command tools; Qwen3.8 primary, Ornith challenger; persisted repository intelligence; build/test verification. Python is permitted only in offline benchmark and research tooling. The runtime has no Python dependency.

Read [MASTER_SPEC.md](MASTER_SPEC.md) first, then `architecture.md`, `data-model.md`, and `agent-loop.md`.
