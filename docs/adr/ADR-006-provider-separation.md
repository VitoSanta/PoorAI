# ADR-006: Provider separation

**Status:** Accepted. **Decision:** isolate all serving APIs behind a provider-neutral domain trait.

The orchestrator consumes normalized inspection, backend state, streaming and error types. Consequence: provider additions require contract fixtures; no provider-specific control flow enters agent policy.
