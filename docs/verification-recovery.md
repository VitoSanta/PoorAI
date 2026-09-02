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
