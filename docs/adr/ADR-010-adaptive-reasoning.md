# ADR-010: Adaptive reasoning policy

**Status:** Accepted. **Decision:** express reasoning level as a versioned `ModelStrategy` parameter, selected by task risk and calibrated resource profile.

**Heuristic:** use lower deliberation for retrieval/simple edits and higher for ambiguous multi-file fixes when measured gains justify it. Consequence: reasoning controls are capability-probed and their effects evaluated per model.
