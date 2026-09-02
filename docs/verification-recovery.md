# Verification and Recovery

Verification is deterministic whenever the repository permits it: formatter/linter, targeted tests, build/typecheck, then broader suite according to policy. Baselines establish pre-existing failures. Each check records command policy ID, environment fingerprint, exit code, bounded logs, timing, and artifact hash.

Recovery taxonomy: compilation/type error, test assertion, tool/environment failure, context/provider failure, policy denial, and non-determinism. For code failures, retrieve diagnostic locations, make one hypothesis-linked correction, rerun the narrow check, then escalation check. For infrastructure failures, do not modify code until the failure is classified. Default budgets: 3 edit-verify cycles and 1 context-tier retry; make configuration explicit.

## Language coverage, and the current violation

"Appropriate to the repository" means any repository. `discover_checks` currently recognises two build systems, and a repository matching neither yields no checks at all — so a run against it completes without verification and reports `verified: false` honestly, while the agent worked blind throughout.

This is a violation of MASTER_SPEC requirement 6 rather than a missing feature, and it is tracked in `direction.md`. No measurement recorded in this repository has been taken on a language other than Rust or JavaScript, so none of the results generalise beyond them.
