# Repository Intelligence

Repository intelligence is a persisted, invalidatable index—not a hidden memory. Record repository root identity, VCS HEAD/diff status, file inventory, ignore rules, language/build manifests, symbols, imports/dependencies, test mapping, and content hashes.

Use incremental updates keyed by content hash and HEAD. Retrieval ranks explicit task terms, path/symbol relations, call/import graph proximity, test ownership, and recent tool evidence. Each retrieved excerpt must retain file path, line range, hash, rationale, and token cost. Respect `.gitignore` plus poorAI policy exclusions for secrets, build outputs, and large/generated files. A stale index must be marked and refreshed before edit decisions.

## Implementation status

Retrieval is **lexical**, and the name matters: it ranks symbol definitions, path components and literal occurrences of the task's terms. It does not understand the code, so the passage it ranks first is the one that mentions the task's words most, which is not always the one that matters most. Every excerpt carries its path, line range, whole-file hash, estimated token cost and the signals that selected it, so a wrong retrieval is diagnosable rather than mysterious.

Weights are named constants rather than inline numbers, so a ranking decision can be argued with. Occurrence counting saturates: a file mentioning a term a hundred times is not fifty times more relevant than one mentioning it twice. An excerpt is centred on the densest matching line rather than the top of the file, which is the difference between showing the evidence and showing an import block.

The excerpt hash is the whole file's, not the fragment's, so an edit guarded by it stays sound after a partial read.

Retrieval spends a fraction of the context budget rather than a fixed number of passages, and stops when the budget is spent rather than exceeding it. Ignored files are absent from the index and so cannot be retrieved — the secret-leak case, one layer further out.

Not implemented: call and import graph proximity, test ownership, and ranking on recent tool evidence. Non-source files compete on the same footing as source, so a lockfile mentioning a common term can rank above an unrelated source file; measured on a sixty-two file workspace the intended file still led by 114 to 16.

## Symbol extraction is Rust-only

The index is specified to record language and build manifests, and symbol extraction currently matches `fn ` and `pub fn `. A Python, Go, Java or C# file therefore contributes no symbols, and retrieval loses its strongest ranking signal — a symbol definition outranks a path match by five to one — precisely on the repositories where the agent knows least.
