# Security and Sandboxing

Threats include prompt-injected repository files, destructive commands, secrets exfiltration, path traversal, malicious tool output, and dependency-install side effects. The repository is untrusted input.

MVP policy: explicit workspace root; deny access outside it; deny network; deny credential stores and home-directory reads; redact high-confidence secret patterns; allowlist commands; cap CPU/time/output; require user approval for dependency changes, VCS history rewriting, package publishing, network activation, or commands with destructive effects. Run tools in a dedicated sandbox/process boundary where supported. Log policy decisions, never secret values. Security posture must be tested with adversarial fixtures.
