# Benchmark Plan

Build a frozen, licensed corpus of small bugfixes, multi-file changes, repository questions, refactors, test failures, and tool-policy attacks. Each task has a base commit, statement, allowed files, hidden/visible verifier, time budget, and contamination/provenance note.

Phase A benchmarks calibration: latency, throughput, memory and failure rate across context ladder. Phase B benchmarks agent quality: verified resolution, regressions, cost proxy, tool actions, and intervention. Compare Qwen3.8 primary versus Ornith challenger first; include all installed models as controls only after capability probe success. Run repeated seeded trials; retain raw traces/redacted artifacts and publish a Markdown plus JSON report. Change one independent variable per experiment.
