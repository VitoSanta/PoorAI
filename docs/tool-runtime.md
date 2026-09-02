# Tool Runtime

Tools are typed capabilities. Requests carry root-relative paths and limits; results carry status, bounded output, duration, artifact references and redaction flags. Tool output is untrusted input to the model and is redacted before it reaches a prompt.

## The surface, as implemented

| Capability | What it does | Guard |
|---|---|---|
| `ReadFile` | Reads a file, or a line window of one | Refuses binaries and anything resolving outside the root |
| `Search` | Literal string search across workspace text files | Bounded matches, redacted excerpts |
| `ListTree` | Bounded file listing | Skips symlinks and policy-excluded directories |
| `ReplaceText` | Replaces one exact, unique occurrence | Hash guard; refuses an ambiguous or absent match |
| `WriteFile` | Creates a file | Refuses to overwrite; approval gate on manifests |
| `ApplyReplace` | Rewrites a whole file | Hash guard; refuses binaries and oversized content |
| `RunCommand` | Runs one allowlisted command | Sandbox, timeout, output cap, no network without a grant |
| `Complete` | Declares the task done | Accepted only if deterministic verification then passes |

`Search` is literal and repository-scoped. There is no web access and no structural or semantic search.

## Reading and editing at scale

`ReadFile` takes `first_line` and `max_lines` and reports the file's total line count, so a file larger than the output bound can be read in windows rather than being cut with nothing to say where. The `artifact_hash` it returns covers the **whole** file, not the window, so an edit guarded by it stays sound after a partial read.

Creation, partial edit and whole-file rewrite are three tools rather than one. An edit carries the hash of what it replaces; a create has nothing to hash. A single tool doing both would put a blind overwrite one missing argument away.

`ReplaceText` refuses a `find` that matches more than once. Two occurrences mean the caller may not have meant the one that would change, and choosing between them is the silent wrong edit the hash guard exists to prevent.

## Execution

`RunCommand` runs under an allowlist, in a sandbox where the platform provides one, with `env_clear` and only `PATH` restored. `HOME` and `TMPDIR` point inside the workspace, so package managers keep caches and config within the boundary instead of the boundary being widened to reach them; that also makes a run hermetic, since nothing downloaded persists into the next one and nothing in the real home is read.

Every attempt is audited before its result propagates, allowed or denied.
