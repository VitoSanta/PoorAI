# Vision and Scope

poorAI makes local coding agents dependable enough to be evaluated as engineering systems. The target user supplies a repository and a task; poorAI selects a bounded configuration, makes inspectable changes, and returns verification evidence.

## In scope (MVP)

Ollama on the local host; macOS-first discovery with portable interfaces; text/tool-capable local models; repository indexing; tool-calling loop; diff-aware edits; commands in a sandbox policy; build/test/lint verification; calibration and benchmark datasets.

## Explicitly out of scope

Multi-tenant hosting, remote providers, autonomous long-running background work, hidden chain-of-thought storage, browser control, unbounded shell access, cross-model automatic routing, and distributed execution. Each requires separate threat modelling and evidence.

## Success criteria

For a locked task corpus and machine snapshot, poorAI must reproduce profile selection, command policy, and recorded outcome. Improvements require statistically reported evaluation deltas with identical or versioned inputs.
