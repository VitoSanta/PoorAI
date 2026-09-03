# CLI Specification

```text
poorai doctor [--json]                  # discover host, Ollama and capability probes
poorai models inspect <model> [--probe] [--timeout-secs N] [--probe-trials N]
                                        # create/show ModelDefinition
poorai calibrate <model> [--ladder ...] # measured context/resource ladder
poorai repo index [PATH]                # build/update repository intelligence
poorai run <TASK> [--model TAG] [--profile CALIBRATION] [--dry-run]
                                        [--approve dependency-change,history-rewrite,publish,network-access]
                                        [--turn-timeout-secs N]
poorai verify [RUN_ID] [--scope targeted|full]
poorai eval run <SUITE> --model <TAG> --profile <CALIBRATION>
                                        [--seed N] [--temperature-milli N]
                                        [--turn-timeout-secs N] [--out-dir DIR]
poorai report <RUN_OR_EVAL_ID> [--format json|md]
```

`models inspect --probe` executes the capability suite against the live deployment. `--timeout-secs` (default 300) bounds a cold load: a load that outruns it is recorded `unknown`, never as an absent capability. `--probe-trials` (default 3) repeats each sampled trial, because a single sample cannot distinguish "unsupported" from "did not happen this time".

`eval run` takes a frozen corpus file and writes a JSON and a Markdown report. `--seed` and `--temperature-milli` both reach the backend: a seed alone does not make sampling reproducible on every deployment, and both are recorded with the report so it says which kind of run it was.

An approval not granted in advance is asked for at the moment it is needed, when a terminal is attached. Where nothing can answer, the run refuses without asking rather than blocking or assuming consent.

A non-dry `run` requires `--model` and `--profile`, and refuses a calibration that no longer matches the model digest, deployment fingerprint, hardware compatibility key or harness revision in force. `--approve` grants effects that reach past the workspace; nothing is granted unless named, and a grant covers only what it names. `--turn-timeout-secs` (default 300) bounds one turn of the action loop, which carries more context each turn than the last.

Commands default to the current repository but require an explicit resolved root in emitted records. `--json` produces schema-versioned machine output. No command performs network access or dependency installation unless explicitly enabled.

A non-dry `run` and an `eval run` also require an active capability artifact for the deployment — one written by `models inspect --probe` whose model digest and deployment fingerprint match what is being addressed, and which observed `chat`, `streaming`, `structured_tools`, `edit`, `cancellation` and `context_boundary`. A tag that Ollama happens to serve is not evidence that the deployment can be driven.

Any command that loads a model takes a host-wide runtime lease, so a second `run`, `calibrate`, `eval` or live probe is refused while the first holds it, naming the operation to wait for. The lease lives outside any repository: two workspaces on one machine contend for the same hardware.

## Where this specification is ahead of the implementation

Two claims here are not yet true, and are kept rather than quietly deleted because they are the intended surface.

**Exit codes.** The specification calls for 0 verified/success; 1 task or verification failed; 2 invalid input; 3 policy denied; 4 provider unavailable; 5 internal error. The implementation returns 0 on success and **4 for every error**, whatever its category. The category is already carried on each error and is what the mapping would use.

**`report --format`.** Only `json` is accepted. `eval run` does write a Markdown report beside its JSON one; `report` does not.
