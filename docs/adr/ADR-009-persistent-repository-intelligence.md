# ADR-009: Persistent repository intelligence

**Status:** Accepted. **Decision:** persist an invalidatable repository index and factual task ledger.

Repeated large-context scans waste resources and are not reliably reproducible. Consequence: indexed facts have hash/provenance, and stale state must refresh before editing.
