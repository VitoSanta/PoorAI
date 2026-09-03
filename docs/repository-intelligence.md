# Repository Intelligence

Repository intelligence is a persisted, invalidatable index—not a hidden memory. Record repository root identity, VCS HEAD/diff status, file inventory, ignore rules, language/build manifests, symbols, imports/dependencies, test mapping, and content hashes.

Use incremental updates keyed by content hash and HEAD. Retrieval ranks explicit task terms, path/symbol relations, call/import graph proximity, test ownership, and recent tool evidence. Each retrieved excerpt must retain file path, line range, hash, rationale, and token cost. Respect `.gitignore` plus poorAI policy exclusions for secrets, build outputs, and large/generated files. A stale index must be marked and refreshed before edit decisions.

## Implementation status

Retrieval is **lexical**, and the name matters: it ranks symbol definitions, path components and literal occurrences of the task's terms. It does not understand the code, so the passage it ranks first is the one that mentions the task's words most, which is not always the one that matters most. Every excerpt carries its path, line range, whole-file hash, estimated token cost and the signals that selected it, so a wrong retrieval is diagnosable rather than mysterious.

Weights are named constants rather than inline numbers, so a ranking decision can be argued with. Occurrence counting saturates: a file mentioning a term a hundred times is not fifty times more relevant than one mentioning it twice. An excerpt is centred on the densest matching line rather than the top of the file, which is the difference between showing the evidence and showing an import block.

The excerpt hash is the whole file's, not the fragment's, so an edit guarded by it stays sound after a partial read.

Retrieval spends a fraction of the context budget rather than a fixed number of passages, and stops when the budget is spent rather than exceeding it. Ignored files are absent from the index and so cannot be retrieved — the secret-leak case, one layer further out.

Not implemented: call and import graph proximity, test ownership, and ranking on recent tool evidence. Non-source files compete on the same footing as source, so a lockfile mentioning a common term can rank above an unrelated source file; measured on a sixty-two file workspace the intended file still led by 114 to 16.

## Symbol extraction

A symbol definition outranks a path match five to one in retrieval, so a language whose declarations are invisible loses the strongest ranking signal precisely where the agent knows the code least. Extraction previously matched `fn ` and `pub fn `, and a Python, Go, Java or C# file contributed nothing.

It now recognises the shape `modifier* keyword Name`, which covers declarations across the languages a repository is likely to be written in without a parser for each: function, class, struct, interface, trait, protocol, record, actor, mixin and the rest. Comments and control flow are excluded, since a comment describing a function is not a declaration of it.

This is deliberately shallow. It finds the name a task is likely to mention, not the program's structure, and it does not resolve imports, calls or test ownership — all of which this document specifies and none of which is implemented.

## Persistence and cost

The index is content-addressed as of 2026-09-03: it is written under the hash of its contents and a write never replaces an existing artifact, where it previously overwrote a single `index.json` on every run. `stale()` exists and is still not consulted on the production path.

What has not changed is the cost. **Every run rebuilds the index from nothing**, walking and reading the whole repository, and retrieval then re-reads every file to score it before opening the selected ones again. That is O(repository bytes) per run, twice, on a workspace the previous run had already read. The incremental update keyed by content hash and HEAD that this document specifies is not implemented, and neither is invalidation against VCS HEAD.

Tools reach files the index deliberately excludes. `Search` and `ListTree` do their own directory walk and skip four known names rather than honouring `.gitignore`, so "ignored files are absent from the index and so cannot be retrieved" holds for retrieval and not for the tool surface.
