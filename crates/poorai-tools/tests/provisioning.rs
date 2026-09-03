//! Fetching and installing a toolchain the host does not have.
//!
//! A derived allowlist cannot name the toolchain a workspace does not yet
//! carry: a task that must install a JDK needs an executable no marker in the
//! repository could have implied. The grant widens that, and these check both
//! halves — what it opens, and what stays shut.

use poorai_tools::{Approval, SandboxPolicy, ToolPolicy, run_command};
use std::path::Path;
use std::time::Duration;

fn policy(root: &Path, approvals: Vec<Approval>) -> ToolPolicy {
    ToolPolicy {
        root: root.to_path_buf(),
        extra_readable: Vec::new(),
        // Deliberately empty: nothing the repository implied.
        allow_commands: vec![],
        output_limit: 64 * 1024,
        timeout: Duration::from_secs(30),
        sandbox: SandboxPolicy::Required,
        approvals,
    }
}

#[tokio::test]
async fn without_the_grant_an_unlisted_executable_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let error = run_command(&policy(root.path(), vec![]), "echo", &["hi".into()])
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("not allowlisted"), "{error}");
}

#[tokio::test]
async fn with_the_grant_an_unlisted_executable_runs() {
    let root = tempfile::tempdir().unwrap();
    let result = run_command(
        &policy(root.path(), vec![Approval::ToolchainInstall]),
        "echo",
        &["hi".into()],
    )
    .await
    .unwrap();
    assert!(result.sandboxed);
    assert_eq!(result.stdout.trim(), "hi");
}

/// What makes the grant defensible is where installs land. A child already runs
/// with HOME inside the workspace, so a toolchain installs into the workspace:
/// the host is not modified, and deleting the workspace undoes it.
#[tokio::test]
async fn an_install_lands_inside_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    let policy = policy(root.path(), vec![Approval::ToolchainInstall]);
    let result = run_command(
        &policy,
        "sh",
        &[
            "-c".into(),
            "mkdir -p \"$HOME/.sdkman\" && printf %s \"$HOME\"".into(),
        ],
    )
    .await
    .unwrap();
    let home = result.stdout.trim();
    let canonical = root.path().canonicalize().unwrap();
    assert!(
        Path::new(home).starts_with(&canonical),
        "HOME was {home}, outside the workspace {}",
        canonical.display()
    );
    assert!(
        canonical
            .join(poorai_tools::SCRATCH_DIRECTORY)
            .join(".sdkman")
            .is_dir(),
        "the install did not land in the workspace"
    );
}

/// Writing outside the workspace stays refused, grant or no grant. The grant
/// widens which executable may run, never where it may write.
#[tokio::test]
async fn the_grant_does_not_widen_where_a_command_may_write() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("escaped.txt");
    let policy = policy(root.path(), vec![Approval::ToolchainInstall]);
    let result = run_command(
        &policy,
        "sh",
        &["-c".into(), format!("echo x > {}", target.display())],
    )
    .await
    .unwrap();
    assert!(result.sandboxed);
    assert!(
        !target.exists(),
        "a granted command wrote outside the workspace"
    );
}

/// A sandbox that confines writes while leaving every read open is one half of
/// an exfiltration. Nothing a run legitimately does needs the host's
/// credentials, so they are denied to every sandboxed run rather than only
/// under a grant.
///
/// Aimed at a path that exists on this host. An earlier version pointed at
/// `~/.ssh`, which is absent here, so it reported "no such file" and passed
/// while a mutant that removed the denial entirely survived it — the same
/// mistake, in the same shape, as a fixture that once aimed at an unreachable
/// public address.
#[tokio::test]
async fn the_hosts_credentials_are_not_readable() {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        eprintln!("skipped: no HOME on this host");
        return;
    };
    // Whichever of the denied paths this host actually has. Testing one that is
    // absent proves nothing.
    let Some(present) = ["Library/Keychains", ".docker/config.json", ".ssh", ".aws"]
        .into_iter()
        .find(|relative| home.join(relative).exists())
    else {
        eprintln!("skipped: this host has none of the denied paths, so nothing is observable");
        return;
    };
    let target = home.join(present);
    let root = tempfile::tempdir().unwrap();

    // First: the path really is readable without a sandbox, or the assertion
    // below would hold for reasons that have nothing to do with the profile.
    let unsandboxed = ToolPolicy {
        root: root.path().to_path_buf(),
        extra_readable: Vec::new(),
        allow_commands: vec!["cat".into(), "ls".into()],
        output_limit: 64 * 1024,
        timeout: Duration::from_secs(30),
        sandbox: SandboxPolicy::Disabled,
        approvals: vec![],
    };
    let control = run_command(&unsandboxed, "ls", &[target.display().to_string()])
        .await
        .unwrap();
    assert!(
        control.exit_code == Some(0),
        "{present} is not readable even unsandboxed, so this proves nothing"
    );

    // Both with the grant and with only the allowlist: the denial is a property
    // of the sandbox, not of which approval is held.
    for approvals in [vec![], vec![Approval::ToolchainInstall]] {
        let granted = !approvals.is_empty();
        let mut policy = policy(root.path(), approvals);
        if !granted {
            policy.allow_commands = vec!["ls".into()];
        }
        let result = run_command(&policy, "ls", &[target.display().to_string()])
            .await
            .unwrap();
        assert!(result.sandboxed);
        assert!(
            result.exit_code != Some(0),
            "granted={granted}: the host's {present} was readable from a sandboxed run: {}",
            result.stdout
        );
    }
}

/// A command is executed directly rather than through a shell, so there is no
/// pipe and no redirection: `args` are arguments, never syntax. That is what
/// stops an argument being reinterpreted as a command, and it is worth keeping
/// — but it left no way at all to feed a program its input.
///
/// Measured: a run downloaded a Go toolchain onto a machine without Go, built a
/// correct program, and then could not test it. `printf ... ./wordfreq` and
/// `bash wordfreq input.txt` were both flattened into arguments, and every
/// attempt to give the program its input failed.
#[tokio::test]
async fn a_command_can_be_given_its_standard_input() {
    let root = tempfile::tempdir().unwrap();
    let policy = policy(root.path(), vec![Approval::ToolchainInstall]);
    let result = poorai_tools::run_command_with_stdin(
        &policy,
        "cat",
        &[],
        Some("the cat the dog THE cat bird"),
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout, "the cat the dog THE cat bird");
}

/// A program that reads to end of input never sees one if the pipe is left
/// open, and the run would time out looking like a hang in the program.
#[tokio::test]
async fn the_input_is_closed_so_a_reader_terminates() {
    let root = tempfile::tempdir().unwrap();
    let mut policy = policy(root.path(), vec![Approval::ToolchainInstall]);
    policy.timeout = Duration::from_secs(10);
    let result =
        poorai_tools::run_command_with_stdin(&policy, "wc", &["-l".into()], Some("a\nb\n"))
            .await
            .expect("a reader that waits for end of input should finish");
    assert_eq!(result.stdout.trim(), "2");
}

/// Passing no input leaves the previous behaviour untouched.
#[tokio::test]
async fn a_command_with_no_input_still_runs() {
    let root = tempfile::tempdir().unwrap();
    let policy = policy(root.path(), vec![Approval::ToolchainInstall]);
    let result = run_command(&policy, "echo", &["hi".into()]).await.unwrap();
    assert_eq!(result.stdout.trim(), "hi");
}
