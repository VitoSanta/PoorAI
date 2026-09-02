# Benchmark Plan

Build a frozen, licensed corpus of small bugfixes, multi-file changes, repository questions, refactors, test failures, and tool-policy attacks. Each task has a base commit, statement, allowed files, hidden/visible verifier, time budget, and contamination/provenance note.

Phase A benchmarks calibration: latency, throughput, memory and failure rate across context ladder. Phase B benchmarks agent quality: verified resolution, regressions, cost proxy, tool actions, and intervention. Compare Qwen3.8 primary versus Ornith challenger first; include all installed models as controls only after capability probe success. Run repeated seeded trials; retain raw traces/redacted artifacts and publish a Markdown plus JSON report. Change one independent variable per experiment.

## Corpora, as they exist

| Suite | What it exercises |
|---|---|
| `m5-frozen-v1` | Eight tasks across all six original kinds, on files of tens of lines |
| `generation-v1` | Building an HTTP API from a specification, scored by a verifier that starts the server |
| `realistic-v1` | A line buried in a 200-function file, a 40-file repository that never names the file to change, a two-part change in a large repository, and an injection buried in `docs/` |
| `vague-v1` | The same defects as bug reports rather than specifications, one with the symptom in a different file from the cause |
| `longhaul-v1` | A rename reaching twelve call sites, sized past the point where the history fits in one context |

`m5-frozen-v1` was written before partial editing and retrieval existed, and every task in it fits in a file small enough that whole-file rewriting works and a repository small enough to list. That is why six campaigns against it never surfaced either limit, and why the later suites exist.

Every hidden verifier is validated in both directions before any model runs it: a correct implementation passes and the untouched workspace fails. A verifier that passes both is not a verifier, and one has already been found that way.
