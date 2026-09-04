//! Process isolation and the approval gates for effects that leave the workspace.

use poorai_tools::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn policy(root: &Path) -> ToolPolicy {
    ToolPolicy {
        root: root.to_path_buf(),
        extra_readable: Vec::new(),
        allow_commands: vec!["sh".into(), "git".into(), "cargo".into(), "echo".into()],
        output_limit: 8192,
        timeout: Duration::from_secs(20),
        sandbox: SandboxPolicy::Preferred,
        approvals: Vec::new(),
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Runtime::new().unwrap().block_on(future)
}

// -------------------------------------------------------------- approvals

#[test]
fn editing_a_dependency_manifest_requires_approval() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("Cargo.toml"), "[package]").unwrap();
    let mut policy = policy(root.path());
    let hash = poorai_domain::hash_bytes("[package]");
    let denied = apply_replace(
        &policy,
        Path::new("Cargo.toml"),
        &hash,
        "[package]\nevil = \"1\"",
    );
    assert!(denied.is_err());
    assert_eq!(
        fs::read_to_string(root.path().join("Cargo.toml")).unwrap(),
        "[package]"
    );

    policy.approvals = vec![Approval::DependencyChange];
    assert!(
        apply_replace(
            &policy,
            Path::new("Cargo.toml"),
            &hash,
            "[package]\nok = \"1\""
        )
        .is_ok()
    );
}

#[test]
fn every_known_manifest_and_lockfile_is_gated() {
    for manifest in [
        "Cargo.toml",
        "Cargo.lock",
        "package.json",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "requirements.txt",
        "pyproject.toml",
        "poetry.lock",
        "go.mod",
        "go.sum",
        "Gemfile",
    ] {
        assert_eq!(
            edit_approval(Path::new(manifest)),
            Some(Approval::DependencyChange),
            "not gated: {manifest}"
        );
        // Nested copies are gated too; the gate is the file, not its depth.
        assert_eq!(
            edit_approval(&PathBuf::from("vendor/sub").join(manifest)),
            Some(Approval::DependencyChange)
        );
    }
    assert_eq!(edit_approval(Path::new("src/main.rs")), None);
}

#[test]
fn history_rewriting_requires_approval() {
    for args in [
        vec!["rebase".to_string(), "-i".into(), "HEAD~3".into()],
        vec!["commit".to_string(), "--amend".into()],
        vec!["reset".to_string(), "--hard".into(), "HEAD~1".into()],
        vec!["filter-branch".to_string()],
    ] {
        assert_eq!(
            command_approval("git", &args),
            Some(Approval::HistoryRewrite),
            "not gated: git {args:?}"
        );
    }
    assert_eq!(command_approval("git", &["status".to_string()]), None);
}

#[test]
fn publishing_and_pushing_require_approval() {
    assert_eq!(
        command_approval("cargo", &["publish".to_string()]),
        Some(Approval::Publish)
    );
    assert_eq!(
        command_approval("npm", &["publish".to_string()]),
        Some(Approval::Publish)
    );
    assert_eq!(
        command_approval("git", &["push".to_string(), "origin".into(), "main".into()]),
        Some(Approval::Publish)
    );
    // A forced push is gated whichever rule catches it first.
    assert!(command_approval("git", &["push".to_string(), "--force".into()]).is_some());
}

#[test]
fn an_ungranted_approval_stops_the_command_before_it_runs() {
    let root = tempfile::tempdir().unwrap();
    let policy = policy(root.path());
    let denied = block_on(run_command(
        &policy,
        "git",
        &["push".to_string(), "origin".into(), "main".into()],
    ));
    assert!(denied.is_err());
}

#[test]
fn granting_one_approval_does_not_grant_another() {
    let root = tempfile::tempdir().unwrap();
    let mut policy = policy(root.path());
    policy.approvals = vec![Approval::DependencyChange];
    assert!(policy.require(Approval::DependencyChange).is_ok());
    assert!(policy.require(Approval::Publish).is_err());
    assert!(policy.require(Approval::HistoryRewrite).is_err());
}

#[test]
fn the_default_profile_grants_nothing() {
    for profile in [PolicyProfile::Safe, PolicyProfile::Development] {
        let policy = profile.build(PathBuf::from("/tmp"));
        assert!(policy.approvals.is_empty());
        assert!(policy.require(Approval::Publish).is_err());
    }
}

// --------------------------------------------------------------- sandbox

#[cfg(target_os = "macos")]
#[test]
fn a_sandboxed_command_cannot_write_outside_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().canonicalize().unwrap().join("escaped.txt");
    let policy = policy(root.path());
    let result = block_on(run_command(
        &policy,
        "sh",
        &[
            "-c".to_string(),
            format!("echo pwned > {}", target.display()),
        ],
    ))
    .unwrap();
    assert!(result.sandboxed, "the fixture did not exercise a sandbox");
    assert!(!target.exists(), "sandbox did not prevent the write");
    assert_ne!(result.exit_code, Some(0));
}

#[cfg(target_os = "macos")]
#[test]
fn a_sandboxed_command_can_still_write_inside_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    let policy = policy(root.path());
    let result = block_on(run_command(
        &policy,
        "sh",
        &["-c".to_string(), "echo ok > inside.txt".to_string()],
    ))
    .unwrap();
    assert!(result.sandboxed);
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(
        fs::read_to_string(root.path().join("inside.txt"))
            .unwrap()
            .trim(),
        "ok"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn the_sandbox_denies_network_when_policy_does() {
    let root = tempfile::tempdir().unwrap();
    let mut policy = policy(root.path());
    policy.allow_commands.push("curl".into());
    policy.timeout = Duration::from_secs(20);
    // The URL is assembled inside the shell so no argument contains a scheme
    // for the allowlist check to catch. What refuses here is the sandbox.
    let result = block_on(run_command(
        &policy,
        "sh",
        &[
            "-c".to_string(),
            r#"s=htt; curl -s --max-time 8 -o /dev/null "${s}ps://example.com""#.to_string(),
        ],
    ))
    .unwrap();
    assert!(result.sandboxed);
    assert_ne!(
        result.exit_code,
        Some(0),
        "network reached from inside the sandbox"
    );
}

/// Build tooling needs a scratch area. It is given one inside the workspace
/// rather than by widening the sandbox to all of $TMPDIR, which would let one
/// task's workspace write into another's.
#[test]
fn a_child_process_gets_its_scratch_directory_inside_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    let policy = policy(root.path());
    let result = block_on(run_command(
        &policy,
        "sh",
        &["-c".to_string(), "printf %s \"$TMPDIR\"".to_string()],
    ))
    .unwrap();
    assert_eq!(result.exit_code, Some(0));
    let reported = std::path::PathBuf::from(result.stdout.trim());
    assert!(
        reported.starts_with(root.path().canonicalize().unwrap()),
        "scratch directory {reported:?} is outside the workspace"
    );
    assert!(reported.is_dir());
}

/// The system temp directory stays outside the boundary.
#[cfg(target_os = "macos")]
#[test]
fn the_system_temp_directory_stays_unwritable_inside_the_sandbox() {
    let root = tempfile::tempdir().unwrap();
    let policy = policy(root.path());
    let target = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join("poorai-should-not-exist.txt");
    let _ = fs::remove_file(&target);
    let result = block_on(run_command(
        &policy,
        "sh",
        &[
            "-c".to_string(),
            format!("echo pwned > {}", target.display()),
        ],
    ))
    .unwrap();
    assert!(result.sandboxed);
    assert!(!target.exists(), "system temp directory was writable");
}

/// Package managers keep caches and config under HOME, so the child's HOME
/// points into its workspace. That keeps downloads inside the boundary and
/// makes a run hermetic; the real home is a different directory and stays
/// unwritable.
#[test]
fn a_child_process_gets_its_home_inside_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    let policy = policy(root.path());
    let result = block_on(run_command(
        &policy,
        "sh",
        &["-c".to_string(), "printf %s \"$HOME\"".to_string()],
    ))
    .unwrap();
    let reported = PathBuf::from(result.stdout.trim());
    assert!(reported.starts_with(root.path().canonicalize().unwrap()));
    assert_ne!(reported, PathBuf::from(std::env::var("HOME").unwrap()));
}

#[cfg(target_os = "macos")]
#[test]
fn the_real_home_directory_stays_unwritable_inside_the_sandbox() {
    let root = tempfile::tempdir().unwrap();
    let policy = policy(root.path());
    // Resolved here rather than read from $HOME inside the child, which now
    // points into the workspace.
    let target = PathBuf::from(std::env::var("HOME").unwrap()).join("poorai-should-not-exist.txt");
    let _ = fs::remove_file(&target);
    let result = block_on(run_command(
        &policy,
        "sh",
        &[
            "-c".to_string(),
            format!("echo pwned > {}", target.display()),
        ],
    ))
    .unwrap();
    assert!(result.sandboxed);
    assert_ne!(result.exit_code, Some(0));
    assert!(!target.exists(), "the real home directory was writable");
}

#[test]
fn a_result_always_records_whether_it_was_sandboxed() {
    let root = tempfile::tempdir().unwrap();
    let mut policy = policy(root.path());
    policy.sandbox = SandboxPolicy::Disabled;
    let result = block_on(run_command(&policy, "echo", &["hi".to_string()])).unwrap();
    // An unsandboxed run must be visibly unsandboxed, never silently so.
    assert!(!result.sandboxed);
}

#[test]
fn a_required_sandbox_fails_closed_when_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let mut policy = policy(root.path());
    policy.sandbox = SandboxPolicy::Required;
    // Non-canonicalisable root: no profile can be built for it.
    policy.root = root.path().join("missing");
    let denied = block_on(run_command(&policy, "echo", &["hi".to_string()]));
    assert!(denied.is_err());
}

// ------------------------------------------------------- network access

/// The project's own policy gates network activation on approval rather than
/// forbidding it. Dependency resolution needs the network; so does
/// exfiltration, and an unattended agent reading an untrusted repository is
/// the case the grant exists to make deliberate.
#[test]
fn the_network_is_closed_until_it_is_granted() {
    let root = tempfile::tempdir().unwrap();
    let mut policy = policy(root.path());
    assert!(!policy.network_allowed());
    policy.approvals = vec![Approval::NetworkAccess];
    assert!(policy.network_allowed());
    // A different grant does not open it.
    policy.approvals = vec![Approval::DependencyChange, Approval::Publish];
    assert!(!policy.network_allowed());
}

#[test]
fn an_ungranted_run_cannot_name_a_url_in_a_command() {
    let root = tempfile::tempdir().unwrap();
    let policy = policy(root.path());
    assert!(
        block_on(run_command(
            &policy,
            "echo",
            &["https://example.com".to_string()]
        ))
        .is_err()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn the_sandbox_opens_egress_only_with_the_grant() {
    let root = tempfile::tempdir().unwrap();
    let fetch = |policy: &ToolPolicy| {
        block_on(run_command(
            policy,
            "sh",
            &[
                "-c".to_string(),
                r#"s=htt; curl -s --max-time 10 -o /dev/null "${s}ps://example.com""#.to_string(),
            ],
        ))
        .unwrap()
    };
    let denied = fetch(&policy(root.path()));
    assert!(denied.sandboxed);
    assert_ne!(denied.exit_code, Some(0), "egress without a grant");

    let mut granted_policy = policy(root.path());
    granted_policy.approvals = vec![Approval::NetworkAccess];
    let granted = fetch(&granted_policy);
    assert!(granted.sandboxed);
    assert_eq!(
        granted.exit_code,
        Some(0),
        "granted egress was still blocked: {}",
        granted.stderr
    );
}

/// A network grant must not become a filesystem grant.
#[cfg(target_os = "macos")]
#[test]
fn a_network_grant_does_not_widen_the_filesystem_boundary() {
    let root = tempfile::tempdir().unwrap();
    let mut policy = policy(root.path());
    policy.approvals = vec![Approval::NetworkAccess];
    let target = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join("poorai-network-grant-probe.txt");
    let _ = fs::remove_file(&target);
    let result = block_on(run_command(
        &policy,
        "sh",
        &[
            "-c".to_string(),
            format!("echo pwned > {}", target.display()),
        ],
    ))
    .unwrap();
    assert!(result.sandboxed);
    assert!(!target.exists());
}

// ------------------------------------------------------------------ reads

/// The sandbox confined writes and left reads open. With `--provision`
/// granting an arbitrary executable and a network together, that is the shape
/// of an exfiltration, and the nine denied credential paths narrowed it rather
/// than closing it.
///
/// This fixture first proves the file is readable *without* the sandbox. Three
/// fixtures in this project have passed for a reason unrelated to what they
/// tested -- one aimed at a `~/.ssh` that did not exist and reported "no such
/// file" as a denial while a mutant removing the rule survived.
#[cfg(target_os = "macos")]
#[test]
fn a_sandboxed_command_cannot_read_a_file_outside_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().canonicalize().unwrap().join("private.txt");
    fs::write(&secret, "the contents nobody outside should see").unwrap();

    // Without the sandbox this succeeds, which is what makes the denial below
    // evidence of the sandbox rather than of a missing file.
    let unsandboxed = ToolPolicy {
        sandbox: SandboxPolicy::Disabled,
        ..policy(root.path())
    };
    let open = block_on(run_command(
        &unsandboxed,
        "sh",
        &["-c".to_string(), format!("cat {}", secret.display())],
    ))
    .unwrap();
    assert_eq!(open.exit_code, Some(0), "the file was not readable at all");
    assert!(open.stdout.contains("nobody outside"));

    let result = block_on(run_command(
        &policy(root.path()),
        "sh",
        &["-c".to_string(), format!("cat {}", secret.display())],
    ))
    .unwrap();
    assert!(result.sandboxed, "the fixture did not exercise a sandbox");
    assert_ne!(result.exit_code, Some(0), "the sandbox read it: {result:?}");
    assert!(
        !result.stdout.contains("nobody outside"),
        "contents leaked: {}",
        result.stdout
    );
}

/// A directory outside the workspace cannot be listed either. Names are
/// evidence on their own -- a repository list, a client list, a filename that
/// says what a person is working on.
#[cfg(target_os = "macos")]
#[test]
fn a_sandboxed_command_cannot_list_a_directory_outside_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside = outside.path().canonicalize().unwrap();
    fs::write(outside.join("acquisition-notes.md"), "x").unwrap();

    let result = block_on(run_command(
        &policy(root.path()),
        "sh",
        &["-c".to_string(), format!("ls {}", outside.display())],
    ))
    .unwrap();
    assert!(result.sandboxed);
    assert!(
        !result.stdout.contains("acquisition-notes"),
        "directory names leaked: {}",
        result.stdout
    );
}

/// The failure mode of a strict profile is that nothing runs at all: denying
/// every read denies the linker its cache and `/usr/bin/true` aborts before
/// `main`. The boundary is only worth having if the toolchain still works, so
/// that is asserted with a real command rather than by reading the profile.
#[cfg(target_os = "macos")]
#[test]
fn the_toolchain_still_runs_under_the_read_boundary() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("in.txt"), "workspace contents").unwrap();
    let result = block_on(run_command(
        &policy(root.path()),
        "sh",
        &["-c".to_string(), "cat in.txt && git --version".to_string()],
    ))
    .unwrap();
    assert!(result.sandboxed);
    assert_eq!(
        result.exit_code,
        Some(0),
        "the read boundary broke the toolchain: {result:?}"
    );
    // The workspace itself stays readable, which is the whole point of the
    // agent being there.
    assert!(result.stdout.contains("workspace contents"));
    assert!(result.stdout.contains("git version"));
}

/// Found by watching a real run spend a fifth of its budget on `ls .poorai`
/// and `cat` of an index artifact before it had installed anything.
///
/// `list_tree` and `search` exclude the harness's state through the shared
/// walker, but a command does not go through that walker. The agent's
/// workspace is the project, not the records the harness keeps about it.
#[cfg(target_os = "macos")]
#[test]
fn a_sandboxed_command_cannot_read_the_harness_state() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join(".poorai");
    fs::create_dir_all(state.join("indexes")).unwrap();
    fs::write(state.join("indexes/idx.json"), "harness bookkeeping").unwrap();
    fs::write(root.path().join("real.rs"), "fn real() {}").unwrap();

    let result = block_on(run_command(
        &policy(root.path()),
        "sh",
        &["-c".to_string(), "cat .poorai/indexes/idx.json".to_string()],
    ))
    .unwrap();
    assert!(result.sandboxed);
    assert!(
        !result.stdout.contains("harness bookkeeping"),
        "the agent read the harness's own records: {}",
        result.stdout
    );

    // The project itself is untouched by the denial.
    let project = block_on(run_command(
        &policy(root.path()),
        "sh",
        &["-c".to_string(), "cat real.rs".to_string()],
    ))
    .unwrap();
    assert_eq!(project.exit_code, Some(0));
    assert!(project.stdout.contains("fn real"));
}

/// The scratch directory is deliberately readable: it is the child's own HOME
/// and TMPDIR, and denying it would break the tools that were pointed at it.
#[cfg(target_os = "macos")]
#[test]
fn the_childs_own_scratch_directory_stays_readable() {
    let root = tempfile::tempdir().unwrap();
    let result = block_on(run_command(
        &policy(root.path()),
        "sh",
        &[
            "-c".to_string(),
            "echo written > \"$TMPDIR/probe.txt\" && cat \"$TMPDIR/probe.txt\"".to_string(),
        ],
    ))
    .unwrap();
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert!(result.stdout.contains("written"));
}
