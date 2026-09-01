# Repository Intelligence

Repository intelligence is a persisted, invalidatable index—not a hidden memory. Record repository root identity, VCS HEAD/diff status, file inventory, ignore rules, language/build manifests, symbols, imports/dependencies, test mapping, and content hashes.

Use incremental updates keyed by content hash and HEAD. Retrieval ranks explicit task terms, path/symbol relations, call/import graph proximity, test ownership, and recent tool evidence. Each retrieved excerpt must retain file path, line range, hash, rationale, and token cost. Respect `.gitignore` plus poorAI policy exclusions for secrets, build outputs, and large/generated files. A stale index must be marked and refreshed before edit decisions.
