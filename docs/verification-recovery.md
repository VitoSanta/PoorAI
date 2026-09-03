# Verification and Recovery

Verification is deterministic whenever the repository permits it: formatter/linter, targeted tests, build/typecheck, then broader suite according to policy. Baselines establish pre-existing failures. Each check records command policy ID, environment fingerprint, exit code, bounded logs, timing, and artifact hash.

Recovery taxonomy: compilation/type error, test assertion, tool/environment failure, context/provider failure, policy denial, and non-determinism. For code failures, retrieve diagnostic locations, make one hypothesis-linked correction, rerun the narrow check, then escalation check. For infrastructure failures, do not modify code until the failure is classified. Default budgets: 3 edit-verify cycles and 1 context-tier retry; make configuration explicit.

## Language coverage

"Appropriate to the repository" means any repository, so check discovery is a registry keyed on marker files rather than a chain of conditions: adding a language is adding a row, and the set of repositories poorAI works in is not decided in this file.

Checks are resolved from three sources, ordered by how directly each speaks for the repository.

**An explicit declaration** at `.poorai/checks.json` is the repository saying how it is verified, and wins.

**Continuous integration configuration** is the repository *doing* it: not a guess about the project but the commands its authors run to check it, and it exists for languages and frameworks nobody here has heard of. GitHub, GitLab, CircleCI, Azure, Jenkins, Travis, Bitbucket and Drone are read as text rather than parsed per vendor, since a parser per vendor would be the same closed list one level down.

Steps are excluded on **effect** rather than vocabulary: anything that deploys, publishes, pushes or reaches the network is not a check whatever it is called, and a step that chains or redirects is a script whose first word would not mean what the file says. Words that usually mark verification are a **preference** for ranking, never a filter — `rebar3 ct` and `zig build test` are both verification, and a list of recognised words closes the world exactly as a list of recognised languages does.

**A marker-file registry** is the fast path where neither exists: cargo, go, maven, gradle, dotnet, swift, flutter, mix, poetry, pytest, bundler, composer, make, ctest, and npm where a test script exists. This is poorAI guessing from a file name, which is why it ranks last.

A repository matching none of the three yields no checks, and the run says it verified nothing rather than completing as though it had passed.

**What is still closed.** A project with no declaration, no CI configuration and no recognised marker cannot be verified, and there is no mechanism yet for the agent to read a README or a Makefile and work out how the project is checked. That is the remaining step toward genuinely any framework, and it carries a question this design has not answered: a check the agent proposes for itself is a command nobody authorised, so it would need the approval path rather than being run because it looked plausible.

The command allowlist is derived from the same detection. A fixed list decides in advance which languages the agent can work in, and a project whose own toolchain is denied cannot be verified at all.

## Recovery, as implemented

**A failing check is reproduced before it is classified.** `classify_with_reproduction` re-runs the failed command and compares the two results, which is what separates a genuine assertion from a flake and from an environment failure. It was written, tested, and until 2026-09-03 never called: the production branch assigned `FailureClass::Assertion` to every failure it saw. The taxonomy above was real in the type system and absent from the loop, so an environment failure authorised an edit — the exact case the paragraph above forbids.

**The budgets come from the execution profile.** Recovery previously constructed `RecoveryBudget::default()` at the call site and passed the run's total action count as the edit attempt count, so reads and searches consumed the edit-verify budget and the profile's declared numbers bound nothing. `ExecutionProfile.budgets` is now parsed as a typed `ExecutionBudgets` and it is what both the loop and recovery spend.

**A context retry steps to a measured tier.** `RetryContextTier` selects the next calibration point below the current context. Where none is lower, recovery stops rather than choosing a smaller number by arithmetic.

**The diagnostics reach the deployment.** Each failing check's command, exit code, both streams, duration, artifact hash and truncation flags are carried into the next turn, rather than the bare fact that something failed.

## No verifier is a failure

A repository matching none of the three sources yields no checks, and a run over it **cannot complete**. It previously recorded `task.complete`, returned success and exited 0, with `verifiable: false` noted beside it; a caller reading the exit code was told a task had succeeded that nothing had checked. The run now refuses the completion and persists `task.failed` naming the absent verifier.

This is deliberately strict, and it costs something real: the two toolchain-provisioning runs recorded in the roadmap built correct programs in workspaces created from nothing, which by construction declare no checks. Under this rule they are failures. That is the honest reading — the deployment verified its own work against a specification and the harness cannot confirm it — and the way out is a verifier the run can be given or asked to approve, not a completion accepted on the deployment's word.

**Closed as of 2026-09-03: a verifier a person adopts.** `propose_verifier` offers a command and runs nothing; the question names the command and the reason. Approved, it becomes a check the run is judged against and its executable joins the allowlist. Refused, the workspace still has no verifier and completion is still refused. It is adopted by the loop rather than by the tool, because a check outlives the action that proposed it, and the adoption is recorded as `verifier.adopted` rather than inferred from the run having succeeded.

Granting `--approve verifier-proposal` in advance lets an unattended run adopt one; without it, a proposal with nobody attached is refused, which is the same rule every other approval follows. Diagnostics are bounded text rather than typed locations, so recovery aims at a paragraph rather than at a file and a line.

## Located diagnostics — 2026-09-03

Check output reached the deployment as bounded prose, so recovery aimed at a paragraph and finding the file and line was work the model paid actions for -- mechanical work, which is the harness's job. rustc, gcc-style and Python traceback shapes are read into a path, a line and a column and travel with the failing check. Deliberately shallow, and shallow safely: a line that does not clearly carry a path and a position is not guessed at, because a wrong location is worse than none -- it sends the agent to edit a file that is fine. The prose is still carried, since a diagnostic the parser did not recognise must not disappear because of it.
