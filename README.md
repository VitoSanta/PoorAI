# poorAI

**Experimental macOS-first local coding agent for Ollama. Public alpha, not production-ready.**

[![CI](https://github.com/VitoSanta/PoorAI/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/VitoSanta/PoorAI/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-2f718e.svg)](LICENSE)
[![Status: public alpha](https://img.shields.io/badge/status-public%20alpha-d97706.svg)](#status)

[Documentation](docs/architecture.md) | [Roadmap](docs/roadmap.md) | [Security](SECURITY.md) | [Contributing](CONTRIBUTING.md)

poorAI runs software-engineering tasks against a repository using an
open-weight model served locally by Ollama. It is written in Rust, and it will
not start a task until it has measured the machine and the model it is about to
use.

Two ideas hold the design together.

**Nothing is inferred that can be measured.** The context a run sends is not
guessed from RAM or read off a model card: it comes from a calibration ladder
that fills each tier with a real prompt, checks a needle planted at its start
came back, and records the generation rate at that occupancy. A run refuses a
calibration that no longer matches the model digest, the deployment, the
hardware or the harness that produced it.

**Nothing is claimed that is not verified.** A task completes only when a
deterministic check appropriate to the repository passes and nothing that was
passing before has broken. A workspace that declares no check cannot complete
at all — the run records a bounded failure naming what was missing, rather than
reporting a success it cannot support.

---

## What it can do today

- Fix a bug, refactor, or answer a question about a repository, verified by the
  repository's own checks.
- Build something from an empty directory, installing the toolchain it needs,
  when the network and toolchain grants are given.
- Work in any language whose checks it can discover: an explicit
  `.poorai/checks.json`, then CI configuration read as text, then a registry of
  fifteen build-system marker files.

## What it cannot do

- **Run unsandboxed platforms safely.** The sandbox is macOS seatbelt. On Linux
  and Windows there is no adapter: a run either refuses or records that it ran
  unconfined. See [SECURITY.md](SECURITY.md).
- **Finish a large multi-file build reliably.** Measured on a fifteen-file PWA
  generated from a specification: the harness improved across three
  configurations and the run still did not finish. `docs/roadmap.md` records the
  numbers.
- **Resume an interrupted run.** State is replayable from the log and shown by
  `poorai session show`, and the loop does not yet start from it.
- **Tell you which model is best.** Routing is declined until comparable
  evidence exists; you name the model.

---

## Requirements

- macOS on Apple silicon. Other platforms build and test, but run unsandboxed.
- Rust 1.88 or newer (the workspace MSRV declared in `Cargo.toml`).
- [Ollama](https://ollama.com) running locally with at least one model pulled.
- Roughly 24 GB of free memory for a 30B model at a 32K context.

## Quick start

```bash
cargo build --release
```

**1. Check the machine and the backend.**

```bash
./target/release/poorai doctor --json
```

**2. Measure what the deployment can actually do.** This runs the capability
suite against the live model and writes an artifact. A run refuses a deployment
without one.

```bash
./target/release/poorai models inspect qwen3.8:27b-mlx --probe --json
```

**3. Calibrate the context ladder.** Each tier is filled with a real prompt, so
this measures what a context costs rather than what can be allocated. Expect it
to take several minutes.

```bash
./target/release/poorai calibrate qwen3.8:27b-mlx --ladder 2048,8192,32768 --json
```

It prints the path of a calibration artifact under `.poorai/calibrations/`.

**4. Run a task**, from inside the repository you want changed:

```bash
poorai run "fix the off-by-one in the range parser" \
  --model qwen3.8:27b-mlx \
  --profile /path/to/.poorai/calibrations/<id>.json \
  --json
```

**5. Read what happened.**

```bash
poorai report <run-id> --format md      # what the run did, in prose
poorai report <run-id> --format jsonl   # the trail, one record per line
```

A run's exit code says which kind of failure it was: `0` verified, `1` the task
or its verification failed, `2` invalid input, `3` a policy denial, `4` the
backend, `5` internal.

## What a run will refuse

Nothing beyond the workspace is granted unless you name it. `--approve` takes
`dependency-change`, `history-rewrite`, `publish`, `network-access`,
`local-service`, `toolchain-install` and `verifier-proposal`; `--provision` is
network plus arbitrary executables together, for installing a toolchain, and
its help says to use it only for work you are willing to watch.

Only one model-loading operation runs at a time, host-wide: a second `run`,
`calibrate` or `eval` waits rather than loading a second 30B model onto a
machine that fits one.

## Evaluation

```bash
poorai eval run corpus/m5-frozen-v1.json \
  --model qwen3.8:27b-mlx --profile <calibration> --seed 1 --seed 2
```

The corpus is frozen data: each task carries a base workspace, allowed files, a
visible verifier and a hidden verifier written only after the agent finishes.
Every proportion is reported with a Wilson interval and its counts. Reports and
a validated `EvaluationRun` are written to `.poorai/evaluations/`.

Results are in [docs/roadmap.md](docs/roadmap.md) and the thresholds they are
judged against in [docs/thresholds.md](docs/thresholds.md) — including a
threshold that currently **fails**, and why it is not being moved to fit.

## Documentation

Start with [MASTER_SPEC.md](MASTER_SPEC.md), which states the binding rules and
where each is enforced. Then:

| | |
|---|---|
| [docs/architecture.md](docs/architecture.md) | crates, the provider boundary, admission |
| [docs/agent-loop.md](docs/agent-loop.md) | the action loop and what it refuses |
| [docs/security-sandboxing.md](docs/security-sandboxing.md) | the boundary in detail |
| [docs/calibration.md](docs/calibration.md) | the ladder and what it measures |
| [docs/evaluation.md](docs/evaluation.md) | the corpus and what a report carries |
| [docs/roadmap.md](docs/roadmap.md) | milestone status, and the measurements behind it |

The roadmap keeps failed experiments, invalidated results and defects found by
running the thing. That is deliberate: a design record that only lists successes
cannot be checked.

## Status

`v0.1.0-alpha.1`. Milestone status is generated from
[docs/milestones.json](docs/milestones.json); do not edit the table in the
roadmap by hand.

## Maintainer

Built and maintained by [Vito Santanelli](https://github.com/VitoSanta).
Use [GitHub Issues](https://github.com/VitoSanta/PoorAI/issues) for reproducible
bug reports and feature proposals; report security concerns through
[SECURITY.md](SECURITY.md).

## Licence

Apache-2.0. See [LICENSE](LICENSE).
