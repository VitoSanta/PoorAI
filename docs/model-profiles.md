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

## ModelProfile: how a deployment is driven

Separate from `ModelDefinition`, which is what the backend reported, and from `ModelStrategy`, which is how the agent behaves. This is how the request is built: context sizes, sampling options and reasoning control, declared per **tag** rather than per family, because the same architecture published under two tags can declare two different limits.

**Every sampling value carries where it came from** — `official_model_card`, `ollama_model`, `poorai_override`, `hardware_calibration`, or `backend_default`. A run that reports a temperature without its origin cannot be compared with another: a value the vendor recommends, one a package happened to ship, and one nobody chose look identical in a report and mean three different things.

Where a vendor recommends nothing, nothing is invented. One deployment's card imposes no sampling recommendation, so its profile sets only what its package declares rather than borrowing another model's `top_k` because it happens to work there.

**Reasoning depth is set three different ways and they are not interchangeable**: a backend option, a line the system prompt must carry, and the backend's own thinking toggle. Each goes to its own channel.

Context is clamped to the tag's declared ceiling. A request for more would be refused or silently ignored, and both make the recorded number a fiction.

### The correction this file exists for

Every measurement recorded in this repository before these profiles existed used `num_ctx = 32768` for all seven deployments and set no sampling at all.

That 32768 came from the M2 ladder, which stopped there because the ladder was chosen rather than measured to a limit. Four of these tags declare 262144 and the rest 131072, so the agent was running at an eighth of the available context on some.

Worse, one deployment declares no parameters in its package, so it ran on the backend's bare defaults while its own card recommends different values. It was the strongest repairer measured here, under a configuration nobody had chosen for it. Comparisons between it and the others were therefore comparisons between configurations as much as between models, and every number recorded before this file should be read with that in mind.

Measured on this machine: `num_ctx` changes the resident footprint on GGUF deployments — one grows from 21 GB at 8K to 25 GB at 131K — and does not move it at all on MLX ones, which matches the M1 finding that MLX deployments do not enforce the limit. All remain fully GPU-resident at every level tested.

### Measured context ladder — 2026-09-02

Four deployments, tiers 32K through their declared ceiling, three samples each, every tier verified fully GPU-resident.

| Deployment | 32K | 65K | 131K | 262K | Cost of context |
|---|---|---|---|---|---|
| ornith-1.5:35b | 70.6 | 70.7 | 70.6 | **71.5** | none |
| nemotron-3.5-lightning | 80.9 | 71.3 | 72.3 | **72.4** | −10%, all of it leaving 32K |
| gpt-oss:20b | 75.6 | 66.7 | **75.8** | — | none |
| qwen3.8:27b-mlx | 27.3 | 20.9 | 19.4 | **18.3** | −33%, monotonic |

Tokens per second, backend-reported. No tier was refused and none was offloaded to the CPU at any size.

**Context is close to free on three of four.** ornith runs at 262K exactly as at 32K. gpt-oss at 131K matches 32K; the 66.7 at 65K carries a standard deviation of 10.1 and is noise rather than a trend, which is worth saying because a table read quickly would show a dip that is not there. nemotron pays its 10% leaving 32K and nothing after, so the larger context is free once that is paid.

qwen is the only one that degrades monotonically. Its 32K figure has a standard deviation of 47.3 against 2.7 at 262K, so it is the least reliable point in the series and the real cost is likely smaller than 33%.

**A claim of mine is withdrawn.** I wrote that `num_ctx` might be inert on MLX deployments, because the resident footprint did not move with context there. Two MLX deployments degrade with context, so the parameter does something the footprint does not show. This is the second time in this session I have generalised across MLX and GGUF and been wrong; whatever divides these deployments, it is not the runtime.

Context defaults now carry `context_source`, so a size measured on this machine does not read like one copied from a specification.

### The context requirement

This agent targets large repositories, so **262144 tokens is a qualification threshold, not a preference**. A deployment that cannot serve it does not qualify for the use case however it scores on a corpus, and one that can is allocated its full ceiling by default. The throughput cost is accepted: a faster deployment that cannot see the repository is not useful here.

| Deployment | Ceiling | Qualifies |
|---|---|---|
| qwen3.8:27b-mlx | 262144 | yes |
| ornith-1.5:35b | 262144 | yes |
| nemotron-3.5-lightning:30b-mlx | 262144 | yes |
| gemma4:31b-mlx | 262144 | yes |
| granite4.2:30b-q6_K | 131072 | no |
| gpt-oss:20b | 131072 | no |
| muse-glimmer:30b-mlx | 131072 | no |

**This costs the best generator.** gpt-oss:20b built a working application in six actions and 1.2 minutes, the fastest of any deployment measured, and its tag tops out at half the required context. Under this requirement it is out, and that is a consequence of the requirement rather than of its performance.

**And it keeps a deployment that does nothing.** gemma4 clears the threshold on context and resolved zero of fifteen edit tasks. Meeting the context requirement is necessary and plainly not sufficient.

The intersection of "can serve 262144" and "does the work" is currently three deployments: qwen, ornith and nemotron.

qwen keeps its recorded price rather than having it erased by the decision: it is the only deployment whose throughput falls monotonically with context, 27.3 tok/s at 32K against 18.3 at 262K, and its 32K figure is the least reliable point in the series. The choice was made against that cost, and a decision that hides its own price cannot be revisited.

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
