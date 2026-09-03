# Security and Sandboxing

Threats include prompt-injected repository files, destructive commands, secrets exfiltration, path traversal, malicious tool output, and dependency-install side effects. The repository is untrusted input.

MVP policy: explicit workspace root; deny access outside it; deny network; deny credential stores and home-directory reads; redact high-confidence secret patterns; allowlist commands; cap CPU/time/output; require user approval for dependency changes, VCS history rewriting, package publishing, network activation, or commands with destructive effects. Run tools in a dedicated sandbox/process boundary where supported. Log policy decisions, never secret values. Security posture must be tested with adversarial fixtures.

## Implementation status

Process isolation uses the macOS seatbelt (`sandbox-exec`): writes are confined to the workspace root by canonical subpath, and network is denied unless policy enables it. Every `ToolResult` records whether it actually ran sandboxed, so an unsandboxed run is never silently one; `SandboxPolicy::Required` refuses to run at all where no sandbox exists. Linux and Windows adapters are not implemented and report unavailable.

Approval gates deny by default and are never inferred: editing a dependency manifest or lockfile, rewriting VCS history, publishing or pushing, and reaching the network each require an explicit grant. Granting one grants only that one.

Approvals can be granted at the moment they are needed, not only in advance. When an action requires one that was not pre-declared, the run asks: the question names the command or the file and the text being changed, because "allow network access" gives a person nothing to judge while "run `git push origin main`" does. A grant is either for that one action or for the run, and a one-time grant expires with the action it was given for. Every decision, including a refusal, is audited with what was asked.

Where nothing is attached to answer, the run refuses without asking. Blocking would hang forever and assuming consent would remove the boundary, so the default is the only safe one, and a grant has to be typed — an empty line is a refusal.

Network access is a grant rather than a prohibition, as this policy always said. It is closed by default because an unattended agent reading an untrusted repository is a different risk from an interactive one with a person approving each step — this project's own corpus contains a repository file instructing the agent to fetch and run a remote script. `ToolPolicy` derives network access from the grant rather than storing it separately, so a run with the network open and no approval recorded cannot be constructed.

A granted run reaches the network and nothing else: the filesystem boundary is unchanged, verified by fixture. Child processes are given `HOME` and `TMPDIR` inside their own workspace, so package managers keep caches and config within the boundary instead of it being widened to reach them. That also makes a run hermetic — nothing downloaded persists into the next run, and nothing in the real home directory is read. A denial surfaces as a confusing error in some tools: npm reports it as root-owned files in its own cache.

## Local services

Verifying a system rather than a file means starting a service and exercising it, which a profile that denies all networking makes impossible. The `LocalService` approval opens that, separately from `NetworkAccess`; neither implies the other, and the audit records which was granted.

It permits binding, listening and connecting on this machine, and denies every remote host — so a service can be started and driven while nothing leaves the machine. A fixture asserts the denial by its error: `PermissionError`, raised by the sandbox refusing the socket, rather than a timeout, which is what an unsandboxed attempt to an unreachable address produces. An earlier version of that fixture aimed at a public address and asserted only that the connection failed; a mutant granting the whole network survived it.

The boundary is the host, not the loopback interface. seatbelt accepts only `*` or `localhost` as the host in a network address — a literal `127.0.0.1` makes the profile fail to compile — and its `localhost` means every address the machine holds. A process under this grant therefore reaches a service listening on a LAN interface, and can be reached from the LAN if it binds there. This is wider than the name suggests, is the platform's limit rather than an intent, and is asserted by its own fixture so that narrowing it later is a visible change rather than a silent one.

## What the policy above overstates — 2026-09-03

**Closed as of 2026-09-03.** Reads were open: nine credential paths were denied and the rest of the machine, home directory included, was legible to any command the agent ran. Reading is now denied by default and opened deliberately — the system paths a process needs to start, the workspace, and the toolchain directories the derived allowlist names. Documents, mail, browser profiles and other repositories are neither readable nor listable.

Three details had to be measured rather than reasoned about. The root directory must be readable as a `literal` or no process starts at all — `/usr/bin/true` aborts before `main`, with no diagnostic. `git` reads its developer directory link from `/private/var/select`. And the denial is on **data**, not on every read: denying metadata denies the path walk that finds the executable, which is a broken sandbox rather than a strict one, while denying data still makes contents unreadable and directories unlistable.

`extra_readable` is the one declared exception, used by corpus preparation for a local mirror the corpus itself names. A task's policy leaves it empty: an agent working in a repository has no business reading elsewhere.

What remains open under `--provision` is narrower but not nothing: an arbitrary executable with a network can still read the system paths and the toolchain directories. Running provisioning in a separate process or VM is the rest of that answer.

**Evaluation was outside this boundary entirely, and is not any more.** Corpus materialisation ran `git` and each entry's declared setup steps, and external tasks ran their verifiers, all through `std::process::Command` with no sandbox, no timeout and no output cap — bypassing everything above from the one place that executes text nobody in this repository wrote. Every such command now runs through `run_command` under a policy of its own: writes confined to the directory being prepared, a wall-clock bound, a bounded output, a process group killed on either, and an allowlist of `git` plus exactly what the corpus declared. A verifier's policy names only the verifier, so one that shells out to something undeclared is refused.

It is a separate policy rather than the run's because preparation needs to fetch a pinned commit and the measured task must not have the network. That grant is asserted by its own fixture, so widening it later is a visible change.

**One host, one model.** Separately from the sandbox, any operation that loads a model takes a host-wide lease before it starts, so two poorAI processes on one machine cannot each load a 30B deployment and produce numbers describing a saturated host. The lease is not a security boundary — it protects the measurement and the machine, not the workspace — but it is the other thing that had `concurrency = 1` written down and nothing enforcing it.
