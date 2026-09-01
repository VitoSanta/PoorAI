# Model Profiles

`ModelDefinition` is facts: canonical ID, digest, family, quantization, modality, inspected limits, tool/chat support, and provenance. It is immutable once stored. `ModelStrategy` is policy: role, prompt form, reasoning mode, tool parallelism, retrieval/edit limits, and verification posture.

| Tag | Initial role | Status |
|---|---|---|
| qwen3.8:27b-mlx | primary optimized | hypothesis, calibrate |
| ornith-1.5:35b | agentic challenger | hypothesis, calibrate |
| granite4.2:30b-q6_K | coding control | laboratory |
| nemotron-3.5-lightning:30b-mlx | long-context control | laboratory |
| gpt-oss:20b | efficiency baseline | laboratory |
| gemma4:31b-mlx / muse-glimmer:30b-mlx | controls | laboratory |

The capability matrix is an **eligibility gate, not a predictor**. Measured across seven deployments, the `edit` probe result has no relationship to how many edit tasks a deployment resolves: the only one that never probed a valid edit is not last, and three of the four that probed perfectly are at the bottom, including one that resolved none. The probe measures whether a deployment can emit an edit the policy accepts, which is a different question from whether it can use that ability to finish a task. Only evaluation answers the second.

No table entry asserts an ability. On discovery, inspect actual serving metadata and execute capability probes: structured tool request, streaming, context boundary, edit task, and timeout/cancellation. Strategies are versioned JSON/TOML and can be promoted only through evaluation.

## Probe requirements

A probe reads the stream to completion. A reasoning deployment opens with `thinking` chunks carrying empty content and emits its tool call near the end — measured at chunk 178 of 178 on one deployment — so a verdict formed from the first chunk reports "no native tool call" for a model that makes one.

Tool calls are read from the typed `ModelChunk::tool_calls` channel. Prose is never re-parsed to infer a call, and a call naming a tool the probe did not offer is not credited.

Emission is sampled behaviour: at least one deployment produces a native call on some runs and not others. Every sampled capability therefore records `trials`, `calls` and `reliable` rather than a boolean, and keeps the failing trials in the record — a rate is not evidence unless the misses are visible. Zero calls in n trials is `unknown`, not proof the deployment cannot make one.

Serving metadata is stored without its tokenizer vocabulary: oversized arrays are replaced by their length and content hash, so the observation stays auditable rather than silently truncated.
