# Model Profiles

`ModelDefinition` is facts: canonical ID, digest, family, quantization, modality, inspected limits, tool/chat support, and provenance. It is immutable once stored. `ModelStrategy` is policy: role, prompt form, reasoning mode, tool parallelism, retrieval/edit limits, and verification posture.

| Tag | Initial role | Status |
|---|---|---|
| qwen3.8:27b-mlx | primary optimized | hypothesis, calibrate |
| ornith-1.5:35b | agentic challenger | hypothesis, calibrate |
| granite4.2:30b-q6_K | withdrawn — too slow to use | laboratory |
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

## Withdrawn deployments

**granite4.2:30b-q6_K — speed.** Measured at 7.4 tokens per second in M2 against 70 for the fastest deployment, and on the `realistic-v1` corpus it took 36.1 minutes per seed where ornith took 1.5 and qwen 7.5 — twenty-four times the slowest of the others. It had already failed the generation suite by exceeding a 900-second per-turn bound without producing one turn. An agent too slow to wait for is unusable in the same way an inaccurate one is, and the cost is paid on every campaign it appears in.

This is a product judgement resting on a measurement, not a capability finding: granite resolved 15 of 24 on the original corpus and is not incapable. It is withdrawn from routine evaluation rather than declared unable.

**gemma4:31b-mlx — does not use the tools.** Zero of fifteen edit tasks resolved, and thirty actions spent on the generation task without creating a file. Its only passes are the adversarial tasks, which pass by not acting.

Retaining a deployment that scores zero has one use — it is a floor, and a harness change that moves it is a change to the harness rather than a flattering model — so it is kept for occasional checks after structural changes rather than dropped entirely.

## ModelStrategy, as implemented

A strategy is policy for one deployment: a suffix appended to the shared system prompt, an action budget, and a retrieval quota. It is selected by an exact match on the model reference, so a near miss gets the shared default rather than someone else's policy. Declared strategies live in `strategies/default.json`; an absent or unreadable file means every deployment gets the shared default, which is what every measurement so far was taken under.

Every declared strategy carries a rationale naming the measurement that prompted it, and a strategy without one is an opinion with a schema.

**Nothing here is evidence.** A strategy is a hypothesis until it is measured against the shared default on a frozen corpus, and the three currently declared have not been. They are written down so they can be tested, not because they are known to help.

The measurements say the deployments differ in ways a strategy could address. One calls `list_tree` on all three trials of an edit probe instead of proposing an edit. One is the strongest repairer and writes a generated server that misses its contract. One is the weakest repairer and builds a working server in six actions. One emits a native tool call on two of three otherwise identical probes. And a single prompt change moved one deployment from 13 of 24 to 20 of 24 while leaving another unmoved at 17 — the same intervention, opposite effects.

A single-prompt harness leaves that on the table. Building it is open work, and a strategy must be measured against the default rather than asserted, or it is an opinion with a schema.
