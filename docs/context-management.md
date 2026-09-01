# Context Management

Context is a budget across system instructions, task, repository excerpts, tool outputs, model response, and safety reserve. The scheduler maintains token estimates plus actual provider-reported values when present.

## Requirement

Context capacity is selected only from compatible `CalibrationProfile` evidence, model inspection metadata, and current backend state. If no safe evidence exists, use the conservative bootstrap profile and label it uncalibrated; do not extrapolate from machine RAM.

## Algorithm

1. Reject incompatible calibration (model digest, backend/version, quantization, hardware compatibility key).
2. Bound requested context by inspected provider/model capability and measured stable calibration points.
3. Reserve output and tool-result headroom; apply repository retrieval quotas by symbol/file relevance.
4. Compact only at explicit checkpoints: persist a factual task ledger, retain source references and hashes, then replace bulky history with summary plus retrievable evidence.
5. On context/backend failure, reduce one calibrated tier, retry once if idempotent, and event it.

Token counting is provider-specific and an estimate unless the backend reports exact counts.
