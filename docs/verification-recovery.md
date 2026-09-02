# Verification and Recovery

Verification is deterministic whenever the repository permits it: formatter/linter, targeted tests, build/typecheck, then broader suite according to policy. Baselines establish pre-existing failures. Each check records command policy ID, environment fingerprint, exit code, bounded logs, timing, and artifact hash.

Recovery taxonomy: compilation/type error, test assertion, tool/environment failure, context/provider failure, policy denial, and non-determinism. For code failures, retrieve diagnostic locations, make one hypothesis-linked correction, rerun the narrow check, then escalation check. For infrastructure failures, do not modify code until the failure is classified. Default budgets: 3 edit-verify cycles and 1 context-tier retry; make configuration explicit.

## Language coverage

"Appropriate to the repository" means any repository, so check discovery is a registry keyed on marker files rather than a chain of conditions: adding a language is adding a row, and the set of repositories poorAI works in is not decided in this file.

Recognised: cargo, go, maven, gradle, dotnet, swift, flutter, mix, poetry, pytest, bundler, composer, make, ctest, and npm where a test script exists.

The registry is not a closed world. A repository declares its own checks at `.poorai/checks.json`, and a declaration wins over the registry, because the repository knows how it is verified and the registry only guesses. A repository matching neither yields no checks and the run says it verified nothing, rather than completing as though it had passed.

The command allowlist is derived from the same detection. A fixed list decides in advance which languages the agent can work in, and a project whose own toolchain is denied cannot be verified at all.
