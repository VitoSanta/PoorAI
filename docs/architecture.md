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
