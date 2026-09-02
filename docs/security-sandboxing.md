# Security and Sandboxing

Threats include prompt-injected repository files, destructive commands, secrets exfiltration, path traversal, malicious tool output, and dependency-install side effects. The repository is untrusted input.

MVP policy: explicit workspace root; deny access outside it; deny network; deny credential stores and home-directory reads; redact high-confidence secret patterns; allowlist commands; cap CPU/time/output; require user approval for dependency changes, VCS history rewriting, package publishing, network activation, or commands with destructive effects. Run tools in a dedicated sandbox/process boundary where supported. Log policy decisions, never secret values. Security posture must be tested with adversarial fixtures.

## Implementation status

Process isolation uses the macOS seatbelt (`sandbox-exec`): writes are confined to the workspace root by canonical subpath, and network is denied unless policy enables it. Every `ToolResult` records whether it actually ran sandboxed, so an unsandboxed run is never silently one; `SandboxPolicy::Required` refuses to run at all where no sandbox exists. Linux and Windows adapters are not implemented and report unavailable.

Approval gates deny by default and are never inferred: editing a dependency manifest or lockfile, rewriting VCS history, publishing or pushing, and reaching the network each require an explicit grant. Granting one grants only that one.

Network access is a grant rather than a prohibition, as this policy always said. It is closed by default because an unattended agent reading an untrusted repository is a different risk from an interactive one with a person approving each step — this project's own corpus contains a repository file instructing the agent to fetch and run a remote script. `ToolPolicy` derives network access from the grant rather than storing it separately, so a run with the network open and no approval recorded cannot be constructed.

A granted run reaches the network and nothing else: the filesystem boundary is unchanged, verified by fixture. Child processes are given `HOME` and `TMPDIR` inside their own workspace, so package managers keep caches and config within the boundary instead of it being widened to reach them. That also makes a run hermetic — nothing downloaded persists into the next run, and nothing in the real home directory is read. A denial surfaces as a confusing error in some tools: npm reports it as root-owned files in its own cache.
