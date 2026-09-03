# Memory and State

Separate ephemeral inference context from durable operational state. Durable SQLite records task/session IDs, events, profiles, repository index manifests, tool artifacts, prompt hashes, verification reports, and references to encrypted/redacted blobs. Raw prompts and source content are opt-in retention with a TTL.

A `TaskLedger` is a compact factual checkpoint: goal, accepted constraints, inspected evidence, edits made, pending hypotheses, verification status, and artifact IDs. It enables restart without pretending the model remembers. State transitions are append-only; derived projections can be rebuilt. Store schema migrations and artifact format versions.

## Implementation status

The durable half exists and the session half does not.

`Store` records the event log — append-only, hash-chained, keyed by run — and `task_ledger` reconstructs the compact factual checkpoint this document describes: files read with their hashes, edits with the hash each produced, refusals with their reasons, and the state of the checks. It is built from the audit rather than from the deployment's recollection, because a model's account of its own work can be wrong and an event log cannot.

**Sessions exist as of 2026-09-02.** `poorai run --session NAME` carries what earlier runs of that name established into the next one, and `poorai session list` / `poorai session show NAME` read them back. A session is a projection of the event log rather than a table beside it, and every file it touched is re-hashed from disk when the ledger is rebuilt, so a file edited outside poorAI between runs is reported as changed rather than replayed at a hash the workspace no longer has. This paragraph previously said every run starts from nothing; that stopped being true.

**The state is recoverable; the loop does not yet start from it.** A session hands the next run a factual summary of what earlier runs did, which is not the same as resuming one. `RunState::replay` now folds a run's typed events back into the state they describe — the machine's position, the actions charged, the files written with their latest hashes, the verifiers a person adopted, the plan and its steps, and the context after any downgrade. A run with no terminal event was interrupted rather than finished, and `session show` says so.

What remains is handing that state to a new run as its starting point instead of zero. Until then, "restart without pretending the model remembers" describes the ledger and the recovered state, not a loop that continues.

Events are typed as of 2026-09-03. The log stored `(&str, Value)` with the type written as a literal at each call site, so nothing checked that two places recording the same event agreed on its shape. `RunEvent` derives both from one value; the stored column is unchanged and older artifacts still read.

**The chain is verifiable and per run as of 2026-09-03.** It was global, so a run's events were hashed over whatever ran between them and two runs in one database could not be verified independently; both chains are now kept. And nothing walked either: SQLite permits `UPDATE` and `DELETE` whatever the API exposes, so append-only was a property of the code and not of the data. `verify_run_chain` recomputes each event's hash from what is stored — an edited payload and a deleted row both break it — and `report` carries the verdict. Rows written before the run chain existed are counted as unlinked rather than treated as intact.

Not implemented: TTL on retained content, encrypted blob references, artifact format versioning, and comparison of a loaded artifact's `schema_version` against the `SCHEMA_VERSION` in force.
