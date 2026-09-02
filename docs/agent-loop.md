# Agent Loop

States: `Discover → Profile → Index → Plan → Act → Verify → {Complete | Recover | Failed}`. Every transition has a typed event and durable checkpoint.

Planning produces a bounded plan: files/symbols to inspect, intended tools, expected checks, and stop conditions. Acting executes one tool call at a time in MVP. The model never receives unrestricted shell authority; it requests a typed action. Before edits, capture a baseline diff and relevant verification baseline. Verification selects declared project checks, interprets results structurally, and recovery applies a bounded diagnosis/edit/verify cycle.

Stop on verified success, policy denial, irreversible ambiguity, budget exhaustion, or repeated non-progress. Do not silently keep trying.

## Action channel

Actions are offered to the deployment as native tools, one per turn: a name and typed arguments, with no prose to fence and no schema to invent. Deployments that emit no native call fall back to a bare JSON action object; fenced or decorated output is refused rather than unwrapped.

A model reply is the whole stream, never its first chunk — a reasoning deployment opens with empty-content `thinking` chunks and its answer arrives later. Every consumer goes through one collector so that mistake has one place to not be made.

A policy denial is returned to the deployment as a tool result rather than ending the run: a refusal such as a stale edit hash is actionable, and discarding work already done because of one is a loss, not a safeguard. The action budget bounds the loop.

Every event of a run shares the run's identifier, from opening provenance through to outcome.

## Repetition

A deployment proposing the same refused action three times is not short of budget; it is not reading the refusal, and more actions buy more repeats. The loop names it: the repetition is recorded as `loop.detected` and the deployment is told plainly that the action will not succeed and what to do instead.

Two attempts are a retry, which can be reasonable — a hash may genuinely have changed. Three is a loop. Repetition is judged on the capability and its target rather than the whole proposal, so a second attempt with a corrected hash is not counted while the same wrong edit twice is, and any successful action clears the streak because a refusal followed by progress is recovery.

This is the measured failure shape of every budget-exhausted run recorded here: the repository already fixed, the deployment still editing.

## Planning

A run may begin with one turn spent asking for a plan, opt-in per deployment strategy. The plan is **context, not authority**: nothing in the loop enforces it, no step grants permission, and verification is unchanged. If it turns out wrong the deployment is told to depart from it.

It is bounded to eight steps, because a longer list is a script rather than a plan, and it costs a turn — which is why it is opt-in and has to be measured against the default rather than assumed to help. A deployment asked for a plan that answers in prose produced none, and that is recorded as a fact about the deployment rather than treated as an error.
