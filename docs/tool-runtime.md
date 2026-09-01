# Tool Runtime

Tools are typed capabilities: `ReadFile`, `Search`, `ListTree`, `ApplyPatch`, `GitDiff`, and `RunCommand`. Requests include declared purpose, root-relative paths, limits, and idempotence. Results contain status, bounded stdout/stderr, duration, exit code, artifact references, and redaction flags.

`RunCommand` uses an allowlisted working root, environment allowlist, timeout, output cap, process-group cancellation, and no network by default. `ApplyPatch` rejects paths outside root, binary edits, conflicts, and unreviewed large changes. Shell command strings are an adapter detail; policy evaluates executable/arguments independently where possible. Tool output is untrusted input to the model and is redacted before prompt inclusion.
