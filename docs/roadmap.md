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
| M2 Calibration | **two deployments calibrated under the occupancy ladder** | The ladder fills the tier, samples pressure after the reply, and records the occupancy the backend reports with whether a needle placed at the start came back. qwen3.8:27b-mlx across 2048-65536: occupancy 0.90-0.95, needle 3/3 at every tier, generation falling from 18.3 to 12.6 tokens per second; 131072 did not answer within the measurement's own timeout and was refused as a stable point. granite4.2:30b-q6_K across 8192-65536: needle 3/3 at every tier and 6.0 falling to 4.2 tokens per second, about a third of qwen's rate at the same occupancy. | Five deployments still carry no profile under this harness. Linux and Windows host probes. |
| M3 Safe execution | **complete on macOS** | Gitignore semantics through one walker shared by the index and the tools; reads denied outside the workspace with a measured allowlist; writes confined; network denied without a grant; bounded incremental I/O with process-group kill; every attempt audited allowed or denied, in five outcome classes; corpus preparation and external verifiers under their own bounded policy; services owned by a supervisor that kills them on drop. | Linux and Windows adapters. Under --provision an arbitrary executable with a network can still read the system and toolchain paths; that wants a separate process or VM. |
| M4 Agent task loop | **complete** | Baseline, typed action, policy-controlled tool, audit, narrow verification, classified recovery, escalation at completion, terminal. Every transition persisted. Completion refused where nothing verifies, and judged against the baseline rather than a green suite. Non-progress detected by what changed rather than by what was proposed. A plan is a graph whose claims are checked where a check exists. Run state is replayable from the log. | The loop does not yet start from a replayed state; the state is recoverable and surfaced, and resuming into it is the step left. |
| M5 Evaluation | **two campaigns stand** | m5-frozen-v1 over 16 runs twice and external-v1 over 4 twice, on qwen3.8:27b-mlx, with per-run cost folded from the events and a validated EvaluationRun beside each report. Every metric carries a Wilson interval. Failures are recorded by class and malformed calls by kind, so a campaign can be read after its workspaces are gone. | The challenger and five controls are unmeasured. The tool-failure metric is split: no broken tools in 34 attempts, every failure a command exiting non-zero, and the bar now attaches to harness_failure_rate, which reads inconclusive at [0.000, 0.102] -- one clean attempt short of clearing. Hidden verification failed at 9 of 10 against a bar of 1.0, and the campaign predates the retention that would let that miss be read. |
| M6 Beta | **not ready** | The thresholds were met by two deployments on a harness that has since changed under three of the metrics it reported. | Re-run the campaign that missed the hidden-verification bar, now that a rejected completion leaves its work behind. Re-derive the tool-failure bar on a corpus large enough to split into a pilot and an evaluation; external-v1's four tasks are not. Measure the challenger. Neither corpus exercises a large context: the peak prompt was 12,080 tokens against 32,768 authorised, so nothing here tests what the calibration measured. Nothing reads thresholds.md, so every verdict in it is computed by hand. Usability beyond --json. |

Campaign evidence: m5-frozen-v1 (16 runs, twice) and external-v1 (4 runs, twice) on qwen3.8:27b-mlx, 2026-09-04, under calibration-harness-v4; granite4.2:30b-q6_K calibrated but not yet evaluated; five controls unmeasured.
<!-- /generated:milestones -->

## What remains

Every item the audit of `cee5ebd` raised has been closed; these are what is
open now, and each says what would settle it rather than when it will happen.

### Before any number here is quoted again

- **Only one deployment has been measured.** `qwen3.8:27b-mlx` has a
  calibration under the occupancy ladder and one campaign. The challenger and
  the five controls have neither, so nothing here compares deployments.
- **The tool-failure threshold is inconclusive, one attempt short.** The nine
  failures were read: `external-v1`, re-run at the same corpus revision with
  the classes recorded, produced no broken tools at all — every failure was a
  command the deployment ran on purpose exiting non-zero. The metric is now
  split, and the bar attaches to `harness_failure_rate`, which measured 0 of
  34. Its interval is `[0.000, 0.102]`, whose upper bound is above the 0.10
  bar, so it is inconclusive rather than met; 35 clean attempts would have
  cleared it. See the 2026-09-04 amendment in [thresholds.md](thresholds.md).
- **The bar cannot be re-derived on the corpus it judges.** A pilot has to be
  disjoint from what it calibrates, and `external-v1` has four tasks — one of
  which the provider failed on. So corpus size, not the derivation rule, is
  what blocks it.
- **A quarter of `external-v1` never ran.** One task of four ended in a
  provider failure with zero turns, `[0.046, 0.699]`. An interval that wide
  says only that it is not obviously rare.
- **No code reads the thresholds.** Every verdict in `thresholds.md` was
  computed by hand from a report. That is the same shape as the counter that
  was initialised and never incremented: nothing fails loudly when it drifts.
- **A threshold of 1.0 can be falsified but never met**, for the same reason a
  threshold of 0 can. A Wilson lower bound is below 1 for any finite run of
  successes, so `hidden verification` and `scope respected` are reported as
  `not falsified` with the bound their clean runs establish. Falsification is
  unaffected, which is why the verdict below stands.
- **A declared completion was rejected by the hidden verifier.** `bugfix-parse`
  at seed 2: the visible test went green, the loop verified it, and the hidden
  check disagreed. Under the interval rule 9 of 10 against a bar of 1.0 is
  `failed`. The specific defect cannot be read — the campaign predates the
  retention that now keeps the agent's work for exactly these outcomes — so it
  needs a re-run before anything can be concluded about it.
- **Malformed calls are `multiple_calls` and `no_tool_call`, evenly.** Six of
  fifty turns, three each, and zero of every other kind. The deployment is not
  misunderstanding the schemas it was given: it is emitting several calls in
  one turn, which the loop refuses whole, or answering in prose instead of
  acting. Whether the loop should take the first of several calls rather than
  discard them all is an open design question, not a defect.
- **The capability probe overstates the action channel.** It records
  `structured_tools` as reliable from three trials of a trivial call. Three
  trials cannot predict a fifth of real turns being unusable, and the probe's
  own documentation says a unanimous three can report a coin flip as a fact.
- **A campaign's classifications die with its workspace.** Reports now carry
  `tool_failures_by_class`, which is why the failures above could be read at
  all — but only for campaigns run after it existed. Anything measured before
  cannot be re-examined, only re-run.
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
