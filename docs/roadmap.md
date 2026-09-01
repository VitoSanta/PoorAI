# Roadmap and Advancement Gates

| Milestone | Deliverable | Advancement criterion |
|---|---|---|
| M0 Foundation | workspace, domain schemas, event log, CLI | compile, clippy, unit tests; schema invariants tested |
| M1 Discovery | Ollama inspect/state + HardwareProfile | captured fixtures and doctor works on target Mac |
| M2 Calibration | ladder harness/profile store | repeatable samples; invalidation tests; no arbitrary capacity default |
| M3 Safe execution | repo index, typed tools, policy | adversarial path/secret/command tests pass |
| M4 Agent task loop | plan/act/verify/recovery | locked smoke corpus completes with recorded evidence |
| M5 Evaluation | benchmark runner/reports | two models evaluated on frozen suite with artifacts |
| M6 Beta | usability/reliability hardening | predeclared success, regression, safety thresholds met |

Threshold values are set before M5 based on baseline measurements. No milestone is advanced merely because a demo works.

## Current implementation status — 2026-09-01

| Milestone | Status | Evidence recorded | What remains before advancement |
|---|---|---|---|
| M0 Foundation | **Completed** | Cargo workspace; versioned domain contracts; SQLite hash-chained event log; JSON CLI. 18 property tests cover serde round-trips, `Observation` variant integrity, deployment fingerprint sensitivity, UUIDv7 ordering, hash determinism, calibration sampling floors and execution-profile authorisation. `cargo fmt --all`, `cargo test --workspace` (71 tests) and `cargo clippy --workspace --all-targets -- -D warnings` all pass. | Maintain the invariant suite as schemas gain fields; every new persisted contract needs its round-trip property. |
| M1 Discovery | **In progress** | `poorai doctor --json` captured the local Mac hardware facts and Ollama `/api/ps`. `models inspect --probe` runs the capability suite against all seven target deployments with a persisted artifact each (see below). Hermetic adapter fixtures cover structural tool-call and thinking parsing, metadata pruning, and NDJSON ordering; probe-policy tests cover drain, trial aggregation and cancellation. | Calibrate the trial count against measured variance (three trials did not reproduce a known intermittency); `context_boundary` and `edit` remain blocked on M2 and M4 respectively. |
| M2 Calibration | **In progress** | Three-sample context ladder, raw artifact hashes, compatible-point selection, persisted calibration artifacts and invalidation gate; profiled runs capture fresh backend state and memory pressure when observable. | Record actual target-model calibration runs; capture backend/runtime snapshots per sample; stable-point pressure and generation-rate metrics; invalidation tests. |
| M3 Safe execution | **In progress** | Persisted repository inventory with basic `.gitignore` exclusions; typed policy; bounded `ReadFile`, `Search` and `ListTree`; hash-guarded text replacement; traversal/binary-file denial; output redaction; timeout; verification baseline. | Support full gitignore semantics; record all tool audit events; adversarial fixture suite for injection, malformed provider replies, stale indexes, flaky verification and timeouts. |
| M4 Agent task loop | **Completed** | Multi-step bounded controller: baseline → typed model action → policy-controlled tool → audit → verification → bounded recovery → terminal event. Hermetic smoke tests cover success and fail → recovery → repair → verified success. | Maintain M4 tests while extending model capabilities; do not expand tool authority without M3 policy/audit coverage. |
| M5 Evaluation | **Not started** | `EvaluationRun` schema exists. | Frozen-corpus loader, reproducible runner, reports and two-model laboratory comparison with artifacts. |
| M6 Beta | **Not started** | — | Predeclare and meet reliability, regression and safety thresholds after M5 measurements. |

### Capability probe results — 2026-09-01

Seven deployments, `--probe-trials 3`, artifacts under `.poorai/*-probe.json`.

| Deployment | structured_tools | Tool-call chunk | cancellation | streaming |
|---|---|---|---|---|
| qwen3.8:27b-mlx | observed 3/3 | 21/22, 30/31, … | observed, 311 ms | observed |
| ornith-1.5:35b | observed 3/3 | 23/23 | observed, 191 ms | observed |
| granite4.2:30b-q6_K | observed 3/3 | 102/102 | observed, 647 ms | observed |
| nemotron-3.5-lightning:30b-mlx | observed 3/3 | 151, 52, 104 | observed, 386 ms | observed |
| gpt-oss:20b | observed 3/3 | 178/178 | observed, 261 ms | observed |
| gemma4:31b-mlx | observed 3/3 | 47/48 | observed, 759 ms | observed |
| muse-glimmer:30b-mlx | observed 3/3 | 80/80 | observed, 752 ms | observed |

`context_boundary` and `edit` are `unknown` by construction: the first needs M2 calibrated limits, the second the M4 typed-action harness. No entry here promotes a model; these are discovery facts only.

**Superseded measurement.** An earlier run of this suite recorded `structured_tools: unknown` for all seven deployments. That was a probe defect, not a model property: the verdict was formed from the stream's first chunk, which for a reasoning deployment carries `thinking` with empty content, while the tool call arrives near the end — at chunk 178 of 178 for gpt-oss. Those artifacts have been replaced.

**Known sampling limitation.** nemotron-3.5-lightning was directly observed producing a native call on one CLI run and not on the next, yet reports 3/3 here. Three trials therefore do not characterise it, and `reliable: true` in that artifact is a sampling artifact rather than a dependability claim. The trial count is not yet calibrated against measured variance, and no reliability threshold has been set — that belongs with the M5 threshold work.

### Schema invariants under property test

The suite is mutation-checked, not just green: relaxing the measured-capacity rule to accept half a stable point, and lowering the stable-point sampling floor from three samples to one, each fail it. The load-bearing properties are that `Observation` cannot round-trip from `unknown` into `observed`, that a deployment fingerprint ignores identity and rotated credentials while tracking anything that changes what is served, and that an execution profile is authorised only when a measured stable point covers the requested context — MASTER_SPEC rule 4, expressed as a test rather than a convention.

### Current safety boundary

`poorai run` supports only `--dry-run`. It does not invoke a model, edit a repository, or claim completion. A non-dry run remains deliberately unavailable until M2/M3/M4 evidence gates are satisfied.
