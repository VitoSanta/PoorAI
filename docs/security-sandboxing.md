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
