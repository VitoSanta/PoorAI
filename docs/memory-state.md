# Memory and State

Separate ephemeral inference context from durable operational state. Durable SQLite records task/session IDs, events, profiles, repository index manifests, tool artifacts, prompt hashes, verification reports, and references to encrypted/redacted blobs. Raw prompts and source content are opt-in retention with a TTL.

A `TaskLedger` is a compact factual checkpoint: goal, accepted constraints, inspected evidence, edits made, pending hypotheses, verification status, and artifact IDs. It enables restart without pretending the model remembers. State transitions are append-only; derived projections can be rebuilt. Store schema migrations and artifact format versions.

## Implementation status

The durable half exists and the session half does not.

`Store` records the event log — append-only, hash-chained, keyed by run — and `task_ledger` reconstructs the compact factual checkpoint this document describes: files read with their hashes, edits with the hash each produced, refusals with their reasons, and the state of the checks. It is built from the audit rather than from the deployment's recollection, because a model's account of its own work can be wrong and an event log cannot.

What is missing is the part that makes it a session. **Every run starts from nothing.** A run has an identifier but no name, no continuity, and no way to be resumed: the ledger is used to compact a history within one run and then discarded with it. "Restart without pretending the model remembers" is what the ledger was built for, and nothing calls it for that yet.

This is the single blocker on work of hundreds of steps, which the largest measured success — 48 actions — is an order of magnitude below. It is also nearly free: the facts are already persisted and already reconstructed, so what is missing is naming a session and reopening it.

Not implemented: TTL on retained content, encrypted blob references, and artifact format versioning.
