# Agent Loop

States: `Discover → Profile → Index → Plan → Act → Verify → {Complete | Recover | Failed}`. Every transition has a typed event and durable checkpoint.

Planning produces a bounded plan: files/symbols to inspect, intended tools, expected checks, and stop conditions. Acting executes one tool call at a time in MVP. The model never receives unrestricted shell authority; it requests a typed action. Before edits, capture a baseline diff and relevant verification baseline. Verification selects declared project checks, interprets results structurally, and recovery applies a bounded diagnosis/edit/verify cycle.

Stop on verified success, policy denial, irreversible ambiguity, budget exhaustion, or repeated non-progress. Do not silently keep trying.

## Action channel

Actions are offered to the deployment as native tools, one per turn: a name and typed arguments, with no prose to fence and no schema to invent. Deployments that emit no native call fall back to a bare JSON action object; fenced or decorated output is refused rather than unwrapped.

A model reply is the whole stream, never its first chunk — a reasoning deployment opens with empty-content `thinking` chunks and its answer arrives later. Every consumer goes through one collector so that mistake has one place to not be made.

A policy denial is returned to the deployment as a tool result rather than ending the run: a refusal such as a stale edit hash is actionable, and discarding work already done because of one is a loss, not a safeguard. The action budget bounds the loop.

Every event of a run shares the run's identifier, from opening provenance through to outcome.

## The conversation

Each turn appends the deployment's own reply to the history before the result of it. This is not a formality. For most of this project's life the loop appended only tool messages, so every request was the system prompt, the task, and a run of results answering nothing — a deployment could not see what it had already proposed, and re-derived the same action from the same unchanged prompt. That single omission produced the dominant measured failure mode, a repository correctly fixed with the completion never declared, in 11 of 48 runs of one campaign and in every deployment tested. Two fixtures now assert that the assistant's turn reaches the history it is sent next, and that no tool result outnumbers the turns it answers; both fail when the append is removed.

A reply that carries structured tool calls and no prose is recorded as its calls rather than as an empty turn, so what was proposed survives in the history either way.

## What the harness knows, the deployment is told

A refusal that withholds what it already knows costs a turn to rediscover. So a stale-hash refusal names the current hash; an edit whose replacement is already in place is reported as already applied rather than as a missing match; and every result carrying a content hash names it `expected_hash`, the name of the parameter that consumes it, as well as under its own. Each of these was measured costing actions in a run that had none to spare.

Every tool result also carries the budget: how many actions remain, and — once deterministic checks are passing — how long they have been passing and how many actions have gone by without a file changing. The loop does not act on these itself; declaring completion stays the deployment's to do.

## Repetition

A deployment proposing the same refused action three times is not short of budget; it is not reading the refusal, and more actions buy more repeats. The loop names it: the repetition is recorded as `loop.detected` and the deployment is told plainly that the action will not succeed and what to do instead.

Two attempts are a retry, which can be reasonable — a hash may genuinely have changed. Three is a loop. Repetition is judged on the capability and its target rather than the whole proposal, so a second attempt with a corrected hash is not counted while the same wrong edit twice is, and any successful action clears the streak because a refusal followed by progress is recovery.

This is the measured failure shape of every budget-exhausted run recorded here: the repository already fixed, the deployment still editing.

## Planning

A plan is loop state, not a message. Pushed once into the history it is context and nothing consults it again; worse, compaction drops it, which on a long task removes the decomposition exactly when it starts to matter. Held as state it survives compaction, its outstanding steps appear in the status of every turn, and it is reconciled against `plan.reconciled` when completion is declared.

Progress is claimed by the deployment through `record_progress`, which records a claim and changes nothing in the workspace. The harness never infers that a step is finished: inferring would be the harness deciding the task had progressed. A claim naming a step the plan does not have is a mistake and is not counted.

The reconciliation is recorded rather than enforced. A plan is not binding and can turn out to be wrong, so completing with steps outstanding is preserved as a fact and never refused on the plan's account.

A run may begin with one turn spent asking for a plan, opt-in per deployment strategy. The plan is **context, not authority**: nothing in the loop enforces it, no step grants permission, and verification is unchanged. If it turns out wrong the deployment is told to depart from it.

It is bounded to eight steps, because a longer list is a script rather than a plan, and it costs a turn — which is why it is opt-in and has to be measured against the default rather than assumed to help. A deployment asked for a plan that answers in prose produced none, and that is recorded as a fact about the deployment rather than treated as an error.

## Malformed tool calls

A call naming a real tool with arguments that do not match its schema is a mistake the deployment can correct, and it can only correct one it is told about. The loop returns the problem as a tool result and continues, rather than ending the run and reporting the harness's silence as the deployment's failure.

Measured before the change: five of thirteen evaluation runs ended this way, three of them the entire generation suite, in each case with actions still unspent.

Three consecutive malformed calls do end the run. A deployment that cannot form a valid call after being told three times what was wrong is not going to, and the budget is better spent failing.
