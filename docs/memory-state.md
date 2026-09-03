# Memory and State

Separate ephemeral inference context from durable operational state. Durable SQLite records task/session IDs, events, profiles, repository index manifests, tool artifacts, prompt hashes, verification reports, and references to encrypted/redacted blobs. Raw prompts and source content are opt-in retention with a TTL.

A `TaskLedger` is a compact factual checkpoint: goal, accepted constraints, inspected evidence, edits made, pending hypotheses, verification status, and artifact IDs. It enables restart without pretending the model remembers. State transitions are append-only; derived projections can be rebuilt. Store schema migrations and artifact format versions.

## Implementation status

The durable half exists and the session half does not.

`Store` records the event log — append-only, hash-chained, keyed by run — and `task_ledger` reconstructs the compact factual checkpoint this document describes: files read with their hashes, edits with the hash each produced, refusals with their reasons, and the state of the checks. It is built from the audit rather than from the deployment's recollection, because a model's account of its own work can be wrong and an event log cannot.

**Sessions exist as of 2026-09-02.** `poorai run --session NAME` carries what earlier runs of that name established into the next one, and `poorai session list` / `poorai session show NAME` read them back. A session is a projection of the event log rather than a table beside it, and every file it touched is re-hashed from disk when the ledger is rebuilt, so a file edited outside poorAI between runs is reported as changed rather than replayed at a hash the workspace no longer has. This paragraph previously said every run starts from nothing; that stopped being true.

**Continuity is not resumption.** A session hands the next run a factual summary of what earlier runs did. It does not resume a run that was interrupted: a crash loses the state the loop was in, and the run that follows starts over with a ledger rather than continuing from a checkpoint. `TaskCheckpoint` is persisted on every production transition as of 2026-09-03, so the events a resume would need are now recorded; what is missing is the reducer that rebuilds loop state from them and a run that starts from that state instead of from zero. Until that exists, "restart without pretending the model remembers" describes the ledger, not recovery from interruption.

**The log is append-only by API, not by enforcement.** `Store` exposes only `append` and chains each event to the last, but SQLite permits `UPDATE` and `DELETE` and nothing walks the chain to verify it. The chain is also global rather than per run, so a run's events are hashed over whatever ran between them.

Not implemented: TTL on retained content, encrypted blob references, artifact format versioning, and comparison of a loaded artifact's `schema_version` against the `SCHEMA_VERSION` in force.
