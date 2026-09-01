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

No table entry asserts an ability. On discovery, inspect actual serving metadata and execute capability probes: structured tool request, streaming, context boundary, edit task, and timeout/cancellation. Strategies are versioned JSON/TOML and can be promoted only through evaluation.
