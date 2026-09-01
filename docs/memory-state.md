# Memory and State

Separate ephemeral inference context from durable operational state. Durable SQLite records task/session IDs, events, profiles, repository index manifests, tool artifacts, prompt hashes, verification reports, and references to encrypted/redacted blobs. Raw prompts and source content are opt-in retention with a TTL.

A `TaskLedger` is a compact factual checkpoint: goal, accepted constraints, inspected evidence, edits made, pending hypotheses, verification status, and artifact IDs. It enables restart without pretending the model remembers. State transitions are append-only; derived projections can be rebuilt. Store schema migrations and artifact format versions.
