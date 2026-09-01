# Agent Loop

States: `Discover → Profile → Index → Plan → Act → Verify → {Complete | Recover | Failed}`. Every transition has a typed event and durable checkpoint.

Planning produces a bounded plan: files/symbols to inspect, intended tools, expected checks, and stop conditions. Acting executes one tool call at a time in MVP. The model never receives unrestricted shell authority; it requests a typed action. Before edits, capture a baseline diff and relevant verification baseline. Verification selects declared project checks, interprets results structurally, and recovery applies a bounded diagnosis/edit/verify cycle.

Stop on verified success, policy denial, irreversible ambiguity, budget exhaustion, or repeated non-progress. Do not silently keep trying.
