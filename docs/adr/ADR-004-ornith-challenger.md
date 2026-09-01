# ADR-004: Ornith challenger

**Status:** Accepted provisionally. **Decision:** `ornith-1.5:35b` is the first comparable challenger.

It runs the same frozen task/evaluation protocol as Qwen3.8. poorAI will not tune benchmark-specific prompts after inspecting hidden test data. Consequence: model differences are reported with profiles, deployment fingerprints, and raw outcome counts.
