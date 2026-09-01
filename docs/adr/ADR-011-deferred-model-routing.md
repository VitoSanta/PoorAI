# ADR-011: Defer automatic model routing

**Status:** Accepted. **Decision:** MVP uses explicit model selection or a configured default; automatic routing is postponed.

Routing has a high risk of circular, benchmark-overfit policy without comparable data. It may be proposed only after calibrated profiles and frozen-suite results cover target models and task classes.
