# Contributing

## Building and testing

```bash
cargo test --workspace
```

Two tests are ignored by default because they need the network. Everything else
is hermetic: no test requires Ollama, a model, or a particular machine.

Before sending anything:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs exactly these and nothing else. Anything needing a model or specific
hardware is run by hand and its results recorded in `docs/`.

## What a change should carry

**A test that fails without it.** Preferably one that fails for the reason the
change exists — several fixtures in this project passed for reasons unrelated
to what they were testing, and each is documented where it happened.

**Behaviour, not source.** A test that greps the source proves the code is
written, not that it runs. Two fixtures here did that, passed, and were
replaced by behavioural ones that failed immediately.

**A comment saying why, where the why is not obvious.** This codebase explains
decisions rather than restating code: what was measured, what was tried, what a
number is for. If a reviewer would ask "why this way?", answer it in the file.

## What the commit message is for

The subject says what changed. The body says what was wrong before and what
evidence supports the change. Where a measurement drove it, give the
measurement. Where a previous version was wrong, say so — the log is the record
of how the design was reasoned about, and a change with no stated reason is one
nobody can revisit.

## Things this project is deliberate about

- **A declared value that nothing reads is a defect.** Several existed and were
  removed; a fixture now fails when a declared value stops reaching the request,
  the policy or the decision it names.
- **The harness does mechanical work; the model decides semantics.** Finding a
  file and line in a compiler's output is the harness's job. Choosing which fix
  to make is not.
- **A refusal carries what it already knows.** A stale-hash refusal names the
  current hash; an ambiguous argument list names the list that should have been
  sent. A refusal that withholds what it has costs a turn to rediscover.
- **No shell interpretation.** An executable and its arguments stay separate,
  and a command line where a program name belongs is refused rather than split.
- **Numbers come with their provenance.** A rate without the counts and the
  interval is not a measurement.
