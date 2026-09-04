# Evaluation

An `EvaluationRun` binds corpus revision, task IDs, harness revision, model/deployment, hardware/snapshot, execution strategy/profile, seeds, time limits, and raw artifacts.

Primary metrics: resolved-task rate, deterministic verification pass rate, regression rate, time-to-verified-result, harness failure rate, intervention count, context/backend failures, and resource footprint. Report median and percentile latency; for proportions report counts and confidence intervals. Never compare runs with changed corpus or verifier without flagging it.

Promotion requires a predeclared comparison, identical environment where practical, and no regression in safety/verification metrics. Qualitative traces diagnose failures but do not substitute for metrics.

## Reproducibility on this host

A recorded seed is not reproducibility. Measured on the local Ollama deployment: passing `seed` alone gives a different answer on every run, while `seed` with `temperature 0` gives byte-identical answers across three runs. The seed reaches the backend only because the request carries it; a harness that records a seed it never sent describes a reproducibility it does not have.

Evaluation runs therefore record both the seed and the temperature. Repeated trials at the backend default temperature measure sampling variance, which is real — one deployment produced a native tool call on two of three otherwise identical probes. Trials at temperature 0 are reproducible but describe a single operating mode, not the one a user runs by default. Neither substitutes for the other, and a report says which it is.

## What a report carries

The task workspace does not survive the run, so anything the run did that is not carried into the report is lost. Each outcome records a count per event type — tool actions, compaction, plans, named loops, malformed calls, approval decisions — so whether the history was compacted or a loop was named is something a reader can see rather than infer.

This gap was found by not being able to answer a simple question about a completed run: a 58-action task finished successfully and there was no way to tell whether compaction had ever run.

## Provenance, as implemented

`EvaluationRun` was the normative contract at the top of this document, and until 2026-09-03 the runner did not build one: it wrote a `SuiteReport`, a parallel shape carrying no run id, no runtime snapshot, no calibration or strategy identity and no harness revision. Comparisons were therefore being made against a contract nothing enforced.

A run now writes a validated `EvaluationRun` beside its report, carrying corpus revision, task set, execution profile, model digest, deployment fingerprint, hardware compatibility key, harness revision, seed, a hash over the outcomes and hashes of both report artifacts. Reports are content-addressed by their own hash and a write refuses to overwrite an existing artifact, so two runs of the same suite no longer collide on a filename.

## Reading the recorded campaigns

**The campaigns in the roadmap predate three fixes that touch what they measured, and have to be re-run before they are quoted again.**

- `tool_failures` was initialised to zero and never incremented. "Zero tool failures in 261 attempts" was arithmetic, not a measurement. A failed attempt is now audited as `failed` distinctly from a policy denial and is counted.
- The malformed-call rate was in the event counts and unreported: 24% of turns on `m5-frozen-v1` and 19% on `external-v1`, roughly one turn in five spent on a call the deployment could not form, against a capability probe calling `structured_tools` reliable on three trials of a trivial call. It is now a metric, and a turn that produces no usable call names its fault — `no_tool_call`, `unknown_capability`, `schema_mismatch`, `argument_shape`, `no_arguments`, `invalid_action`, `unparsed_output` — because prose where a call was expected, an invented capability and a real one filled in wrongly need different fixes. The kind is decided where the fault is found, not recovered afterwards by matching on the message.
- A failure count alone could not be acted on. `external-v1` measured nine and, a day later, could not say whether they were tools breaking or commands the agent ran on purpose exiting non-zero — the workspaces, and the event stores in them, do not survive the run. Reports now carry `tool_failures_by_class`, and the rate is split three ways: `tool_failure_rate` unchanged and without a bar, `harness_failure_rate` for tools that broke, which is what the threshold judges, and `command_failure_rate` for red tests, reported and deliberately not bounded.
- The context sent to the backend was the model profile's static default rather than the calibrated one, so a campaign describing a 32768-token profile may have been running at 262144.
- A task in a workspace with no checks completed successfully. Under the current rule it fails.

## Still open

`--seed` repeats, so a campaign of several trials is one invocation under one runtime lease. A provider failure is read from the terminal event's recorded class rather than from the error text. A `PolicyAttack` needs the legitimate work done as well as the attack refused — resolving it by the absence of a violation rewarded a deployment that did nothing at all. Token counts, backend generation time, generation rate, peak prompt against the authorised context, turns under memory pressure, and loop, non-progress and downgrade counts are folded from each run's own events.

Still open: time to first token, and peak resident memory rather than the host's pressure state. The workspace is still destroyed with the run, so what is not carried into the report is still lost — the trail can now be exported as JSONL before that happens, and the runner does not yet do it automatically.
