# CLI Specification

```text
poorai doctor [--json]                  # discover host, Ollama and capability probes
poorai models inspect <model> [--probe] [--timeout-secs N] [--probe-trials N]
                                        # create/show ModelDefinition
poorai calibrate <model> [--ladder ...] # measured context/resource ladder
poorai repo index [PATH]                # build/update repository intelligence
poorai run <TASK> [--model TAG] [--profile CALIBRATION] [--dry-run]
                                        [--approve dependency-change,history-rewrite,publish]
                                        [--turn-timeout-secs N]
poorai verify [RUN_ID] [--scope targeted|full]
poorai eval run <SUITE> --model <TAG>
poorai report <RUN_OR_EVAL_ID> [--format json|md]
```

`models inspect --probe` executes the capability suite against the live deployment. `--timeout-secs` (default 300) bounds a cold load: a load that outruns it is recorded `unknown`, never as an absent capability. `--probe-trials` (default 3) repeats each sampled trial, because a single sample cannot distinguish "unsupported" from "did not happen this time".

A non-dry `run` requires `--model` and `--profile`, and refuses a calibration that no longer matches the model digest, deployment fingerprint, hardware compatibility key or harness revision in force. `--approve` grants effects that reach past the workspace; nothing is granted unless named, and a grant covers only what it names. `--turn-timeout-secs` (default 300) bounds one turn of the action loop, which carries more context each turn than the last.

Commands default to the current repository but require an explicit resolved root in emitted records. Exit codes: 0 verified/success; 1 task or verification failed; 2 invalid input; 3 policy denied; 4 provider unavailable; 5 internal error. `--json` produces schema-versioned machine output. No command performs network access or dependency installation unless explicitly enabled.
