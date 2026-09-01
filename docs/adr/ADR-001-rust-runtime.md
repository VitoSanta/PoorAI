# ADR-001: Rust production runtime

**Status:** Accepted. **Decision:** implement runtime, CLI, providers, tools, storage, and verification in Rust. Python is confined to offline research/benchmark scripts.

Rust provides explicit resource handling, portable binaries, strong concurrency primitives, and type-safe boundaries for untrusted data. This is a design choice, not a performance claim until benchmarked. Consequence: provider contracts and schemas must be Rust-first; research output imports through stable JSON/CSV artifacts.
