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
| `FetchUrl` | Fetches one http or https URL as text | Requires the network grant; refuses other schemes and redirects |
| `Complete` | Declares the task done | Accepted only if deterministic verification then passes |

`Search` is literal and repository-scoped. There is no structural or semantic search.

`FetchUrl` is a **fetch, not a search**: there is no index and no query, so a caller must already know the address. Naming it search would promise something it does not do. Redirects are refused rather than followed, because a redirect can change scheme or host after the scheme check and would make that check advisory rather than binding. A fetched page is untrusted input in the strongest sense — a remote party wrote it — so it is bounded, redacted and hashed exactly like a file read, and it grants nothing: a page instructing the agent to run a command is prose, and the command still has to pass policy.

## Reading and editing at scale

`ReadFile` takes `first_line` and `max_lines` and reports the file's total line count, so a file larger than the output bound can be read in windows rather than being cut with nothing to say where. The `artifact_hash` it returns covers the **whole** file, not the window, so an edit guarded by it stays sound after a partial read.

Creation, partial edit and whole-file rewrite are three tools rather than one. An edit carries the hash of what it replaces; a create has nothing to hash. A single tool doing both would put a blind overwrite one missing argument away.

`ReplaceText` refuses a `find` that matches more than once. Two occurrences mean the caller may not have meant the one that would change, and choosing between them is the silent wrong edit the hash guard exists to prevent.

## Execution

`RunCommand` runs under an allowlist, in a sandbox where the platform provides one, with `env_clear` and only `PATH` restored. `HOME` and `TMPDIR` point inside the workspace, so package managers keep caches and config within the boundary instead of the boundary being widened to reach them; that also makes a run hermetic, since nothing downloaded persists into the next one and nothing in the real home is read.

Every attempt is audited before its result propagates, allowed or denied.

## Hashes and refusals

An edit is guarded by the hash of the file as it is on disk. Every result that carries such a hash reports it twice: under its own name (`new_hash` after an edit, `artifact_hash` after a read) and under `expected_hash`, which is the name of the parameter the next call must pass it as. One value under two names is redundant; one value under two names where only one of them matches the parameter is a mapping the caller has to infer, and a measured run never made that inference — it re-sent the pre-edit hash four times across two intervening re-reads.

Refusals carry what the refusal already knew. A stale hash names the hash the file now has. An edit whose `find` text is absent *and* whose `replace` text is present is reported as already applied, because "not found" is true but sends the caller round the loop again on work that is done.

## The command allowlist

The allowlist is derived from the repository — the executables named by an explicit `.poorai/checks.json`, by CI configuration, or by the build systems whose markers are present — never a fixed list. Common aliases travel with what a repository declares: `python3` admits `python` and the reverse, `pytest` and `poetry` admit the interpreter they run under, `npm` admits `node` and `npx`, `flutter` admits `dart`. A project whose declared check runs `python3` denying `python` refuses the interpreter it already permits, and did cost a measured run an action.
