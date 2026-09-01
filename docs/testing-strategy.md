# Testing Strategy

Use Rust unit tests for pure domain logic, property tests for profile validation and budget invariants, integration tests with a fake provider and fixture repository, contract tests against a version-pinned Ollama test environment, and end-to-end tests against the local laboratory as separately marked non-hermetic tests.

Test failure modes: malformed provider data, changing backend state, cancellation, context errors, stale index, binary/ignored paths, prompt injection, command timeout, flaky tests, and restart at every checkpoint. Snapshot only stable JSON schemas; avoid snapshots of timings. CI runs hermetic tiers; hardware calibration and real-model evaluation run explicitly with captured environment reports.
