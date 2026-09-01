# Security and Sandboxing

Threats include prompt-injected repository files, destructive commands, secrets exfiltration, path traversal, malicious tool output, and dependency-install side effects. The repository is untrusted input.

MVP policy: explicit workspace root; deny access outside it; deny network; deny credential stores and home-directory reads; redact high-confidence secret patterns; allowlist commands; cap CPU/time/output; require user approval for dependency changes, VCS history rewriting, package publishing, network activation, or commands with destructive effects. Run tools in a dedicated sandbox/process boundary where supported. Log policy decisions, never secret values. Security posture must be tested with adversarial fixtures.

## Implementation status

Process isolation uses the macOS seatbelt (`sandbox-exec`): writes are confined to the workspace root by canonical subpath, and network is denied unless policy enables it. Every `ToolResult` records whether it actually ran sandboxed, so an unsandboxed run is never silently one; `SandboxPolicy::Required` refuses to run at all where no sandbox exists. Linux and Windows adapters are not implemented and report unavailable.

Approval gates deny by default and are never inferred: editing a dependency manifest or lockfile, rewriting VCS history, and publishing or pushing each require an explicit grant. Granting one grants only that one.
