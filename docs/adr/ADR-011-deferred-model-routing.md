# ADR-011: Defer automatic model routing

**Status:** Accepted. **Decision:** MVP uses explicit model selection or a configured default; automatic routing is postponed.

Routing has a high risk of circular, benchmark-overfit policy without comparable data. It may be proposed only after calibrated profiles and frozen-suite results cover target models and task classes.

## Amendment — 2026-09-02: the condition was met, and the answer is still no

This decision made itself revisitable once calibrated profiles and frozen-suite results covered the target models and task classes. Both now exist, across seven deployments and six task kinds, so the question is answerable rather than deferred.

**Automatic routing is declined, and the reasoning is now evidence rather than caution.**

**A router needs to classify a task, and every proxy tried here has failed.** The M1 capability probe does not predict repair: the only deployment the edit probe never observed editing is not last, and three of the four that probed perfectly are at the bottom. Repair does not predict generation: the strongest repairer writes a server that misses its specification, and the second-weakest builds a working one in six actions. Throughput does not predict quality, though it does bound feasibility. A task-type classifier would be the fourth such proxy, trained on this project's own task categories — which is the benchmark-overfit failure this decision was originally taken to avoid, arrived at from the other direction.

**The product requirement removes the case for it.** The agent targets repositories requiring 262144 tokens of context, and the deployment that would have been routed to for generation tops out at half that. The pairing that motivated routing is not available.

**The measured gains are in the harness, not in model choice.** Returning a malformed tool call to the deployment instead of ending the run recovered 38% of evaluation runs. Re-running the narrow check after an edit moved one deployment from 13 of 24 to 20 of 24. Partial editing and retrieval turned a 409-line file and a 62-file repository from unreachable into three or four actions. No difference between deployments measured here is of that size, and the strongest repairer was measured under sampling parameters nobody had chosen for it.

**Two things are kept instead.** Explicit selection per invocation already exists, costs nothing, and puts the choice with the person who knows what they are about to do. And escalation within a deployment — more reasoning effort or a larger action budget when a task proves harder than expected — addresses the same need without a classifier, since difficulty is observable in the audit rather than predicted from a task's category. `ModelStrategy` can express it.

**What would reopen this.** A measured, repeated, interval-separated advantage for one deployment on a task class, where the classification is available before the work starts rather than inferred after it. Nothing measured so far comes close, and the intervals over three seeded trials overlap even between the best and worst of the qualifying deployments.
