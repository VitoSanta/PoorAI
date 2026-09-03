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

## Bounds that hold while output is being produced

A limit applied after the fact bounds the report, not the process. Command output, HTTP bodies and file reads were each materialised whole and truncated afterwards, so a command printing without end was bounded only by the machine. stdout and stderr are now drained incrementally with bounded retention, and the result still carries the hash of the **whole** output and a flag saying it was truncated — the caller learns that something was cut, and the hash still identifies what was produced.

A timeout kills the process group rather than the process. A child that outlives the tool it was spawned by is a process still writing to the workspace after the run stopped watching, which the hash guard on the next edit would then blame on the workspace being stale.

`FetchUrl` streams under a byte cap rather than reading a body to completion, and the provider's NDJSON reader decodes UTF-8 across chunk boundaries instead of per chunk, with a cap on a single line. Decoding per chunk corrupted any code point that spanned two of them.

## What the surface still does not have

One walker serves the index and the tools as of 2026-09-03, so `.gitignore` excludes a file from a tool result exactly as it excludes it from retrieval, and the listing is sorted rather than in directory order.

`MakeDirectory`, `DeletePath`, `MovePath`, `VcsStatus` and `VcsDiff` closed the reorganisation gap on 2026-09-03. A delete carries the hash of what it removes, as an edit does; a directory has to be named recursively and reports how much went; a move refuses an existing destination and will not follow a symlink out of the workspace. The two version-control tools are read-only by construction — no argument they take reaches a mutating subcommand.

What is still missing is a multi-hunk patch: a change touching several places in one file is several whole-file rewrites, each carrying the whole file.

The sandbox confines writing and denies the network; it does not confine **reading**. Nine known credential paths are denied and everything else on the host is legible to a sandboxed command. Under `--provision`, which grants an arbitrary executable and a network together, that is the shape of an exfiltration and the flag's help says so.

`git clean` now needs the same approval as `reset --hard`: both discard uncommitted work, and only one of them was checked.

A tool outcome is one of five — `allowed_success`, `allowed_failure`, `policy_denial`, `timeout`, `protocol_failure` — rather than allowed or not. A command that ran and exited non-zero used to be recorded exactly like one that worked, which is what made the evaluation's failure rate meaningless.

`ToolCapability` is an enum left over from an earlier design and no longer corresponds to the typed actions above. It is two vocabularies for one concept, and one of them is wrong.

Evaluation used to be the exception to all of this. Corpus materialisation and external verifiers shelled out directly, with no sandbox, timeout or output cap; they now run through `run_command` under their own bounded policy. See `security-sandboxing.md`.

## Services — 2026-09-03

`StartService` and `StopService` stand a long-running process up and take it down. `run_command` cannot: it waits for the process to exit, which a server does not do.

Ready means accepting a connection, not existing. A port is reserved from the operating system unless one is named. A start that never answers is stopped rather than left running, and every service is killed when the run ends by any route -- the supervisor's `Drop` is the mechanism, and a mutant removing it fails the fixture.
