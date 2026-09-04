# Roadmap

What poorAI is now, and what is left. The measurements behind every line here
are in [experiment-log.md](experiment-log.md), which keeps the failed
experiments and invalidated results as well as the ones that held.

## Current implementation status

Generated from [milestones.json](milestones.json) by `scripts/milestones.py`.
Milestone state used to be asserted in three places and they disagreed — the
same milestone declared complete and in progress in one file. Edit the
manifest; do not edit the table. CI fails if they drift apart.

<!-- generated:milestones -->
<!-- Generated from docs/milestones.json by scripts/milestones.py. Edit the manifest, not this table. -->

| Milestone | Status | Evidence recorded | What remains before advancement |
|---|---|---|---|
| M0 Foundation | **complete** | Cargo workspace; versioned domain contracts; SQLite event log with a per-run hash chain and a verifier; typed run events; JSON, Markdown and JSONL reporting. Property tests cover serde round-trips, Observation variant integrity, deployment fingerprint sensitivity, UUIDv7 ordering, hash determinism, calibration sampling floors, execution-profile authorisation, typed execution budgets and event round-trips. | Real migrations beyond the two in place, and an artifact table. |
| M1 Discovery | **complete for the seven target deployments** | doctor captures host and backend facts. models inspect --probe runs the full capability suite with a content-addressed, non-overwritable artifact each. A run requires a matching artifact, and the measured emission rate shapes how much malformed-call patience it gets. Cancellation closes the connection the backend writes into, asserted from the server's side. | The trial count is not calibrated against measured variance. |
| M2 Calibration | **one deployment calibrated under the occupancy ladder** | The ladder fills the tier, samples pressure after the reply, and records the occupancy the backend reports with whether a needle placed at the start came back. Measured on qwen3.8:27b-mlx across 2048-32768: occupancy 0.90-0.94, needle recalled 3/3 at every tier, and generation falling from 18.3 to 15.4 tokens per second -- a 16% cost the allocation ladder could not see. | Six deployments still carry no profile under this harness. Linux and Windows host probes. |
| M3 Safe execution | **complete on macOS** | Gitignore semantics through one walker shared by the index and the tools; reads denied outside the workspace with a measured allowlist; writes confined; network denied without a grant; bounded incremental I/O with process-group kill; every attempt audited allowed or denied, in five outcome classes; corpus preparation and external verifiers under their own bounded policy; services owned by a supervisor that kills them on drop. | Linux and Windows adapters. Under --provision an arbitrary executable with a network can still read the system and toolchain paths; that wants a separate process or VM. |
| M4 Agent task loop | **complete** | Baseline, typed action, policy-controlled tool, audit, narrow verification, classified recovery, escalation at completion, terminal. Every transition persisted. Completion refused where nothing verifies, and judged against the baseline rather than a green suite. Non-progress detected by what changed rather than by what was proposed. A plan is a graph whose claims are checked where a check exists. Run state is replayable from the log. | The loop does not yet start from a replayed state; the state is recoverable and surfaced, and resuming into it is the step left. |
| M5 Evaluation | **one campaign stands** | m5-frozen-v1 over 16 runs and external-v1 over 4, on qwen3.8:27b-mlx, with per-run cost folded from the events and a validated EvaluationRun beside each report. Every metric carries a Wilson interval. | The challenger and five controls are unmeasured. The tool-failure threshold was derived from a rate that was never measured and now fails on real repositories; it has no verdict until the nine observed failures are read and the bar is re-derived on the corpus it judges. |
| M6 Beta | **not ready** | The thresholds were met by two deployments on a harness that has since changed under three of the metrics it reported. | Read the nine tool failures and re-derive that bar. Measure the challenger. Decide how many trials constitute a result. Neither corpus exercises a large context: the peak prompt was 12,080 tokens against 32,768 authorised, so nothing here tests what the calibration measured. Usability beyond --json. |

Campaign evidence: m5-frozen-v1 (16 runs) and external-v1 (4 runs) on qwen3.8:27b-mlx, 2026-09-04, under calibration-harness-v4; the challenger and the five controls are unmeasured.
<!-- /generated:milestones -->

## What remains

Every item the audit of `cee5ebd` raised has been closed; these are what is
open now, and each says what would settle it rather than when it will happen.

### Before any number here is quoted again

- **Only one deployment has been measured.** `qwen3.8:27b-mlx` has a
  calibration under the occupancy ladder and one campaign. The challenger and
  the five controls have neither, so nothing here compares deployments.
- **The tool-failure threshold has no verdict.** It was derived from a pilot
  rate of 0.000 that was never measured, and the metric now reads 0.214 on real
  repositories with the whole interval above the bar. The nine observed
  failures have to be read — a non-zero exit from a test the agent ran on
  purpose is not a broken tool — and the bar re-derived on a corpus that can
  produce them. See [thresholds.md](thresholds.md).
- **How many trials constitute a result is unstated.** No threshold says, and a
  single trial of four tasks gives intervals wide enough to be worth little.

### Capability

- **A large multi-file build does not finish.** Measured on a fifteen-file PWA:
  the harness improved across three configurations — writes from none to nine,
  reads down by two thirds, the build from not running to producing real
  errors — and the run still did not complete. Whether that is the harness or
  the deployment is answered by running the same task on another of the seven.
- **The loop does not start from a replayed state.** `RunState::replay` rebuilds
  a run's state from its log and `session show` displays it; resuming into it is
  the step left.
- **Linux and Windows have no sandbox adapter**, so on those platforms a run
  either refuses or records that it ran unconfined.
- **`--provision` grants an executable and the network together.** The pair is
  what installs a toolchain and also what an exfiltration is made of. Running it
  in a separate process or VM is the rest of that answer.

### Structural debt

Declared rather than hidden, and none of it blocks an alpha.

- `poorai-orchestrator/src/lib.rs` is past five thousand lines and
  `poorai-cli/src/main.rs` past three and a half thousand. The CLI still holds
  orchestration — hardware probing, profile resolution, prompt construction,
  the evaluation runner — that belongs behind the orchestrator's boundary.
- The event log's chain is per run and verified, but there is no artifact
  table, and migrations are two `execute_batch` calls rather than a mechanism.
- A persisted artifact's `schema_version` is compared, and there is no
  migration path: an older artifact is refused rather than upgraded.

### Work the harness should absorb from the model

The division this project should hold to: the deployment decides semantics — what is wrong, what to change, which fix is right — and the harness does the mechanical work. Several things on the model's side of that line today are mechanical, and each one costs actions from a budget meant for thinking.

Manifest and dependency discovery, finding the call sites and tests related to a file, ranking and de-duplicating what goes into the context, token accounting, selecting which checks a change requires, correlating a diagnostic to a file and a line, generating a diff, counting edits and recovery attempts, classifying a failure, reproducing a flaky one, detecting that a run has stopped progressing, and retrying at a lower context tier — all of these are the runtime's to do, and several are the P1 and P2 rows above under a different name.

Two boundaries are deliberate. **Which semantic correction to make stays the deployment's**, and inferring that a plan step is finished stays out of the harness — the harness recording that work happened would be the harness doing it. **Whether the conclusion is accepted is the harness's**, and that one moved on 2026-09-03: a completion is now refused where nothing can verify it.
