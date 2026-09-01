# Roadmap and Advancement Gates

| Milestone | Deliverable | Advancement criterion |
|---|---|---|
| M0 Foundation | workspace, domain schemas, event log, CLI | compile, clippy, unit tests; schema invariants tested |
| M1 Discovery | Ollama inspect/state + HardwareProfile | captured fixtures and doctor works on target Mac |
| M2 Calibration | **Completed** | Seven deployments calibrated on the local Mac across a 2048-32768 ladder, three repetitions per tier, tier order shuffled from a recorded seed. Per-tier warm-up verified by backend-reported load duration, generation rate from exact `eval_count`/`eval_duration`, memory pressure and backend state captured per sample, raw samples persisted with every profile. Profiles carry the thresholds they were judged against; refusals are persisted with the criteria that produced them. Invalidation tested across all four declared keys. | Fill the context to measure a loaded tier, not only an allocated one (see below); Linux and Windows host probes. |
| M3 Safe execution | **Completed** | Full gitignore semantics via the ripgrep `ignore` walker with poorAI policy exclusions layered on top. Every tool attempt audited inside the hash chain, denied as well as allowed. macOS seatbelt process isolation confining writes to the workspace and denying network, with the boundary recorded on every result. Approval gates for dependency manifests, history rewriting and publishing, granted by nobody by default. Non-deterministic checks detected by reproduction so a flake cannot authorise an edit. 56 adversarial fixtures. | Linux and Windows sandbox adapters; a CLI path for a user to actually grant an approval, needed once non-dry runs exist. |
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
| M3 Safe execution | **In progress** | Full gitignore semantics (negation, anchoring, `**`, directory-only patterns, nested files, character classes) delegated to the ripgrep `ignore` walker, with poorAI policy exclusions layered on top. Every tool attempt is audited, denied as well as allowed, inside the hash chain. 33 adversarial fixtures cover traversal, symlink escape, secret redaction, command allowlist, network denial, output bounds, timeout, stale hashes, binary files, malformed proposals and prompt injection. | Malformed-provider-reply and flaky-verification fixtures; sandbox/process boundary for tool execution; approval gates for dependency changes, history rewriting and publishing. |
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

### Safety boundary under adversarial test

Every suite is mutation-checked rather than merely green:

| Mutation | Fixtures that fail |
|---|---|
| ignore-rule evaluation disabled | 8 of 10 gitignore |
| audit restored to success-only | 5 of 6 audit |
| approval gate removed from edits | 1 of 12 sandbox/approval |
| sandbox never applied | 3 of 12 sandbox/approval |
| non-determinism never detected | 1 of 11 malformed/flaky |

Two defects were found while closing this milestone. `execute_action` propagated every tool error with `?` before appending to the log, so the event stream held successes only — a policy denial is the boundary doing its job and the event most worth having. And `FailureClass::NonDeterminism` was unreachable: nothing could construct it, so a flaky check classified as `Assertion`, which authorises an edit-and-retry cycle. The agent would have edited working code to chase a failure that was never in the code. A failing check is now re-run, and an outcome that changes on identical inputs stops recovery instead of licensing an edit.

The sandbox is real and measured, not declared: a seatbelt-confined command cannot write outside the workspace, can write inside it, and cannot reach the network — verified against an unsandboxed control that does reach it. `ToolResult.sandboxed` records whether the boundary was actually in force, so an unsandboxed run is visibly unsandboxed. `SandboxPolicy::Required` fails closed where no sandbox exists. Only the macOS adapter is implemented; Linux and Windows report unavailable rather than pretending.

That audit defect was real and is worth recording: `execute_action` propagated every tool error with `?` before appending to the log, so the event stream held successes only. A policy denial is the boundary doing its job and the event most worth having — the log could not previously show that anything had ever been refused. Denials are now written before the error propagates, and the hash chain covers them.

### Calibration results — 2026-09-01

Ladder 2048/4096/8192/16384/32768, seed 42, three repetitions per tier, models unloaded between deployments. Every tier of every deployment was admitted; no sample was measured cold; every rate came from backend-reported token counts.

| Deployment | Median first token | Generation rate | Worst tier spread |
|---|---|---|---|
| nemotron-3.5-lightning:30b-mlx | 39-47 ms | 59-69 tok/s | 18 ms |
| ornith-1.5:35b | 61-63 ms | 70-71 tok/s | 2 ms |
| gpt-oss:20b | 104-130 ms | 54-64 tok/s | 11 ms |
| qwen3.8:27b-mlx | 125-147 ms | 17-23 tok/s | 40 ms |
| granite4.2:30b-q6_K | 132-140 ms | 7.4-7.6 tok/s | 23 ms |
| muse-glimmer:30b-mlx | 201-301 ms | 15-16 tok/s | 41 ms |
| gemma4:31b-mlx | 378-432 ms | 13-16 tok/s | 82 ms |

**What this measures, and what it does not.** The fixed prompt is 74 tokens, so a 32768-token tier allocates that much KV cache and then runs a short generation. The ladder therefore answers whether a deployment can be configured and served at a context size without failing or causing memory pressure — which is what context bounding needs — and not what it costs to run with that context full. Throughput is near-flat across tiers for exactly this reason; reading it as "context size is free" would be wrong. Measuring a loaded tier needs a ladder of prompt sizes, not only of `num_ctx` values.

Generation rate is a laboratory throughput fact and carries no claim about task quality. A deployment ten times faster than another may still resolve fewer tasks; that comparison belongs to M5 and must not be anticipated from this table.

**Two harness artifacts were found and removed by measuring rather than assuming.** A single warm-up per run left every tier after the first carrying a model reload, because the backend reloads when `num_ctx` changes: ~1.7s first-token against ~11ms warm, on every GGUF deployment while MLX deployments showed nothing. The median hid it and the variance inherited it, so affected tiers recorded a spread near 700,000 on samples whose real spread was single-digit milliseconds. And running seven 30B deployments back to back left earlier models resident, so free memory fell below the declared floor and the last deployment was refused for pressure that the battery order had created. Deployments are now unloaded between calibrations, and warm-up is per tier.

### Current safety boundary

`poorai run` supports only `--dry-run`. It does not invoke a model, edit a repository, or claim completion. A non-dry run remains deliberately unavailable until M2/M3/M4 evidence gates are satisfied.
