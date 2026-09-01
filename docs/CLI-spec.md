# CLI Specification

```text
poorai doctor [--json]                  # discover host, Ollama and capability probes
poorai models inspect <model>           # create/show ModelDefinition
poorai calibrate <model> [--ladder ...] # measured context/resource ladder
poorai repo index [PATH]                # build/update repository intelligence
poorai run <TASK> [--model TAG] [--profile ID] [--dry-run]
poorai verify [RUN_ID] [--scope targeted|full]
poorai eval run <SUITE> --model <TAG>
poorai report <RUN_OR_EVAL_ID> [--format json|md]
```

Commands default to the current repository but require an explicit resolved root in emitted records. Exit codes: 0 verified/success; 1 task or verification failed; 2 invalid input; 3 policy denied; 4 provider unavailable; 5 internal error. `--json` produces schema-versioned machine output. No command performs network access or dependency installation unless explicitly enabled.
