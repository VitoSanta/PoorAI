# Direction

Where poorAI is going, and how far it is. Each target names what would have to exist, so a reader can tell a missing capability from an unmeasured one.

## The three behaviours this is aimed at

### 1. Debug an unfamiliar repository from a vague report

*"The cart total looks wrong on a 100 euro order"* against a repository nobody wrote for us.

**Measured**: the vague half works. On `vague-v1` the hidden verifier passed on every trial, including the task where the symptom is in one file and the cause in another. A precise specification was not doing the work.

**Missing**: the repository half. Verification recognises two build systems, symbol extraction recognises one language, and retrieval matches literal words. On a Python or Go repository the agent edits without being able to tell whether it fixed anything — which is not a weaker version of this behaviour but a different and worse one, because it cannot know it failed.

### 2. Write a project's documentation

**Missing entirely, and not for want of tools.** `read_file` and `write_file` are enough to produce the files. The gap is that this whole system rests on deterministic verification, and prose has none: the agent would write, declare completion, and the report would honestly say `verified: false`.

Building this means answering what a documentation task is checked against. Until that has an answer, producing documentation is something poorAI can do and cannot stand behind, and it should not be offered as a capability.

### 3. Build a multi-component system

*"A MaaS with .NET microservices, Docker and Angular."*

**Far off, and the distance is structural rather than a matter of tuning.**

| Needed | Today |
|---|---|
| Decomposition into sub-goals that are then executed | a plan of at most eight steps, which is context and not authority — nothing runs it |
| Hundreds of actions across a session | largest measured success: 48 actions |
| Any toolchain the project uses | an allowlist of specific executables |
| Verification of a running multi-service system | none |
| Continuity across sessions | none; every run starts from nothing |

The honest present tense: poorAI repairs bugs and builds a single application from a specification. It does not plan a system.

## The ordered work

1. **Language agnosticism.** A normative requirement already, and currently violated. Below.
2. **Resumable sessions.** A task of hundreds of steps is not expressible in a runtime where every run starts from nothing. The event log already holds the facts and `task_ledger` already reconstructs them; what is missing is naming a session and resuming it.
3. **Real decomposition.** A plan that is executed rather than offered, with sub-goals carrying their own verification.
4. **Verification for systems, not files.** A multi-service target is checked by standing it up and exercising it, which the generation suite already does for one process and nothing does for several.
5. **Usability.** Named sessions with their repository, branch, accumulated diff and status — the half of M6 untouched. Worth building on top of resumable sessions and worthless without them.

## Routing is declined, not deferred

ADR-011 deferred automatic model routing until comparable data existed. It does now, and the answer is no: every proxy tried in this project has failed on contact, the product's context requirement disqualifies the deployment routing would have selected for generation, and the measured gains are an order of magnitude larger in the harness than in model choice.

What replaces it is escalation within a deployment — noticing from the audit that a task is proving hard and spending more on it — which needs no classifier and no second model resident.

## The language violation

**Closed as of 2026-09-02; this section is kept as the record of what was wrong.** Check discovery reads an explicit declaration, then CI configuration, then a marker registry of fifteen build systems; symbol extraction recognises declaration keywords across eight languages; the command allowlist is derived from the repository. `roadmap.md` carries the measurement. The last paragraph of this section still holds: results measured on Rust and JavaScript repositories do not generalise until they are measured elsewhere, and `external-v1` is the only non-toy corpus so far.

MASTER_SPEC requirement 6 says verification must be *appropriate to the repository*, and `repository-intelligence.md` requires the index to record *language/build manifests*. Both mean any repository. Three places assume otherwise:

- **`discover_checks`** recognises `Cargo.toml` and a `package.json` with a test script. Everything else yields no checks, so an agent working on it cannot tell whether it succeeded.
- **Symbol extraction** matches `fn ` and `pub fn `, which is Rust. A Python file contributes no symbols to the index, so retrieval loses its strongest ranking signal exactly where it is most needed.
- **The command allowlist** names specific executables, so a project's own toolchain is denied unless it happens to be one of them.

The fix is a declarative registry of build systems keyed on marker files, a symbol extractor covering the common declaration forms, and an allowlist derived from what the repository actually is. A repository that matches nothing must be able to declare its own checks rather than being silently unverifiable.

Until then, results on Rust and JavaScript repositories do not generalise, and no measurement in this repository has been taken on any other language.

## Where this document is behind — 2026-09-03

Items 1 and 2 of the ordered work are closed and the table above is stale in three rows: continuity across sessions exists (`--session`, `session list`, `session show`), any toolchain the project declares is now derivable rather than fixed, and the largest measured success is 33 actions provisioning a toolchain from nothing rather than 48 on a corpus task.

What the audit of `cee5ebd` changed about the ordering is item 3. "Real decomposition" was written as the next thing to build; it now sits behind work that is less interesting and more load-bearing. A subgoal graph is worth building on a runtime whose state can be resumed, whose context is the one it measured, and whose completion means something — and until 2026-09-03 none of those three was true on the path a run actually took. The order is now: re-measure the corpus on the current harness, then resumable state, then decomposition on top of it. `roadmap.md` carries that as a backlog with what would settle each item.

Item 5, usability, is unchanged and still last, with one addition: everything poorAI reports is `--json`, and the exit code is 4 for every failure whatever its cause. A caller scripting around poorAI cannot currently tell a policy denial from a provider being down.
