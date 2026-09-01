# ADR-002: Ollama-first provider

**Status:** Accepted. **Decision:** MVP ships one local Ollama adapter behind `ModelProvider`.

It matches the installed laboratory and lets poorAI validate end-to-end contracts early. Ollama HTTP/API behaviours are verified against the installed version and fixtures; native response types stay in adapter code. Consequence: no second provider before provider contract and evaluation evidence exist.
