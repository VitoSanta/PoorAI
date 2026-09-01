# Contribution Guidelines

Open an issue/design note before changing domain schemas, security policy, provider traits, or benchmark rules. Keep commits narrow; add tests for every behaviour change; update an ADR for durable architectural decisions. Do not add runtime Python, hidden network calls, model-name capability assumptions, or unmeasured performance claims.

PRs state requirement/heuristic/fact classification, affected data migrations, benchmark impact, security implications, and verification commands/results. Never commit model weights, repository secrets, raw private code traces, or credentials. Reviewers require deterministic tests and benchmark evidence for optimization claims.
