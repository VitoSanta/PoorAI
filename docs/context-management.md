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

## Measured boundary behaviour

A configured context limit is not a limit the backend can be trusted to enforce. Measured across seven local deployments at the same boundary, three distinct contracts appeared: the limit ignored and the whole prompt accepted; the prompt silently truncated with no error and content lost; and a clean typed rejection. The division is not by runtime -- two deployments on the same backend format behaved differently.

The scheduler therefore enforces the budget before sending, and checks the backend's reported prompt token count against what it believed it sent. A deployment's measured `context_boundary` observation says which contract it offers; a `truncated_silently` deployment gives no other signal that context was dropped.

## Compaction, as implemented

Compaction happens at an explicit checkpoint between actions, when the estimated history exceeds half the context budget — never mid-action, when the history is incomplete.

**The ledger is built from the audit, not from the deployment's recollection.** A summary a model writes about its own work can be wrong about what it did; the event log cannot. The ledger lists files read with their artifact hashes, files changed with their current hashes, commands run with exit codes, actions that were refused and why, and the state of the repository checks after the last change.

Carrying hashes through matters: an edit planned before compaction is still valid after it. Carrying refusals matters for the opposite reason — without them a deployment retries a denied action from a blank memory.

The system prompt and the original task survive compaction because they are the instruction and the goal. Everything between them is reconstructible from the audit and is not worth its tokens.

Token counts here are estimates at four characters per token, labelled as such in the `context.compacted` event. A backend that reports real counts is used where it does; this is not one of those places.
