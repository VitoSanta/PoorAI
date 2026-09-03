# Architecture

## Layers

`poorai-cli` parses commands and renders reports. `poorai-orchestrator` owns the state machine. `poorai-domain` owns schemas and invariants. `poorai-provider` exposes model/provider traits. `poorai-ollama` implements local HTTP. `poorai-tools` enforces tool policy. `poorai-repo` indexes and edits workspaces. `poorai-verify` runs deterministic checks. `poorai-store` persists SQLite/event artifacts. `poorai-observe` emits structured telemetry.

Dependencies point inward: adapters depend on domain interfaces; domain has no HTTP, filesystem, or Ollama dependency. Tokio, serde, tracing, reqwest, sqlx/rusqlite, and clap are candidates, subject to security review.

## Provider boundary

```rust
#[async_trait::async_trait]
pub trait ModelProvider: Send + Sync {
  async fn inspect(&self, deployment: &DeploymentDescriptor) -> Result<ModelInspection>;
  async fn runtime_state(&self) -> Result<BackendState>;
  async fn chat(&self, request: ModelRequest) -> Result<ModelStream>;
}
```

Provider code maps native schemas into stable domain types; it must not leak Ollama JSON into planning. The orchestrator accepts only a validated `ExecutionProfile`.

## Where the layering actually stands

Two departures from the description above are worth naming rather than leaving for a reader to discover.

**`poorai-cli` is the larger orchestrator.** It carries hardware probing, capability-evidence loading, calibration persistence, execution-profile resolution, prompt construction, session handling, the evaluation runner and reporting. `poorai-orchestrator` owns the action loop and the task state machine, which is less than "the state machine" implies. The dependency direction is sound and the crate boundaries hold; the volume is in the wrong crate.

**There are two production-shaped paths.** `run_single_action` sits beside the action loop with its own verification and terminal handling, so a rule added to the loop has to be remembered here — the refusal to complete without a verifier had to be written twice. One of them should go.

**`poorai-observe` emits nothing.** It is seven lines and no crate depends on it. Telemetry in practice is the hash-chained event log in `poorai-store`. See `observability.md`.

## Admission

Nothing loads a model without first taking `ModelRuntimeLease`, a host-wide lock created by atomic file creation outside any repository. Ollama will accept a second client while the hardware will not hold a second 30B deployment, and two workspaces on one machine are two processes with no other way to see each other. The lease records the operation holding it, is reclaimed when its owning process is gone, and is released on drop.

Profile selection then takes the `RuntimeSnapshot` as an input rather than recording it as a fact: an otherwise compatible profile is refused where the host is observably under memory pressure. Before 2026-09-03 the snapshot was captured, stripped of the loaded models it had just read from `/api/ps`, and never consulted.
