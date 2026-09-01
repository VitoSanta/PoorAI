# Data Model

```text
ModelDefinition { id, digest, family, quantization, capabilities, metadata, observed_at }
ModelStrategy { id, model_selector, role, prompting, reasoning, tool_policy, retrieval_policy }
DeploymentDescriptor { id, provider, endpoint, model_ref, backend_options, auth_ref? }
HardwareProfile { id, compatibility_key, os, cpu, accelerators, memory, storage, probe_version }
RuntimeSnapshot { id, hardware_id, deployment_id, timestamp, available_memory, pressure, loaded_models, backend_state }
CalibrationProfile { id, compatibility_key, model_digest, deployment_fingerprint, harness_rev, stable_points, raw_artifacts }
ExecutionProfile { id, strategy_id, calibration_id?, context, reserves, concurrency, budgets, rationale }
EvaluationRun { id, corpus_rev, task_set, execution_profile_id, snapshots, seeds, outcomes, artifacts }
```

IDs are UUIDv7; content uses SHA-256/BLAKE3 hashes; timestamps UTC RFC 3339. Compatibility keys deliberately exclude personal identifiers. Validation prevents `ExecutionProfile` references to incompatible calibration and prevents an evaluation from losing corpus/verifier provenance.
