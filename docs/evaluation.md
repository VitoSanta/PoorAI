# Evaluation

An `EvaluationRun` binds corpus revision, task IDs, harness revision, model/deployment, hardware/snapshot, execution strategy/profile, seeds, time limits, and raw artifacts.

Primary metrics: resolved-task rate, deterministic verification pass rate, regression rate, time-to-verified-result, tool failure rate, intervention count, context/backend failures, and resource footprint. Report median and percentile latency; for proportions report counts and confidence intervals. Never compare runs with changed corpus or verifier without flagging it.

Promotion requires a predeclared comparison, identical environment where practical, and no regression in safety/verification metrics. Qualitative traces diagnose failures but do not substitute for metrics.
