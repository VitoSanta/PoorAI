# Evaluation

An `EvaluationRun` binds corpus revision, task IDs, harness revision, model/deployment, hardware/snapshot, execution strategy/profile, seeds, time limits, and raw artifacts.

Primary metrics: resolved-task rate, deterministic verification pass rate, regression rate, time-to-verified-result, tool failure rate, intervention count, context/backend failures, and resource footprint. Report median and percentile latency; for proportions report counts and confidence intervals. Never compare runs with changed corpus or verifier without flagging it.

Promotion requires a predeclared comparison, identical environment where practical, and no regression in safety/verification metrics. Qualitative traces diagnose failures but do not substitute for metrics.

## Reproducibility on this host

A recorded seed is not reproducibility. Measured on the local Ollama deployment: passing `seed` alone gives a different answer on every run, while `seed` with `temperature 0` gives byte-identical answers across three runs. The seed reaches the backend only because the request carries it; a harness that records a seed it never sent describes a reproducibility it does not have.

Evaluation runs therefore record both the seed and the temperature. Repeated trials at the backend default temperature measure sampling variance, which is real — one deployment produced a native tool call on two of three otherwise identical probes. Trials at temperature 0 are reproducible but describe a single operating mode, not the one a user runs by default. Neither substitutes for the other, and a report says which it is.

## What a report carries

The task workspace does not survive the run, so anything the run did that is not carried into the report is lost. Each outcome records a count per event type — tool actions, compaction, plans, named loops, malformed calls, approval decisions — so whether the history was compacted or a loop was named is something a reader can see rather than infer.

This gap was found by not being able to answer a simple question about a completed run: a 58-action task finished successfully and there was no way to tell whether compaction had ever run.
