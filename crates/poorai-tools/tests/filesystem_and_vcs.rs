//! The surface was read, create and replace.
//!
//! A task that reorganises files could not be expressed at all: the agent
//! could write a file into a new directory but never make an empty one, move
//! anything, or remove what it had superseded. And it could not see its own
//! accumulated change -- it had to remember every file it had touched, and a
//! hash is not a diff.

use poorai_tools::*;
use std::fs;
use std::path::Path;
use std::time::Duration;

fn policy(root: &Path) -> ToolPolicy {
    ToolPolicy {
        root: root.to_path_buf(),
        extra_readable: Vec::new(),
        allow_commands: vec![],
        output_limit: 64 * 1024,
        timeout: Duration::from_secs(20),
        sandbox: SandboxPolicy::Disabled,
        approvals: Vec::new(),
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Runtime::new().unwrap().block_on(future)
}

#[test]
fn a_directory_can_be_made_and_a_path_moved_into_it() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("old.rs"), "body").unwrap();
    let policy = policy(root.path());

    make_directory(&policy, Path::new("src/inner")).unwrap();
    assert!(root.path().join("src/inner").is_dir());

    let moved = move_path(&policy, Path::new("old.rs"), Path::new("src/inner/new.rs")).unwrap();
    assert_eq!(moved.from.as_deref(), Some("old.rs"));
    assert!(!root.path().join("old.rs").exists());
    assert_eq!(
        fs::read_to_string(root.path().join("src/inner/new.rs")).unwrap(),
        "body"
    );
}

/// The same rule `write_file` follows, for the same reason: a blind overwrite
/// should never be one missing argument away.
#[test]
fn a_move_onto_an_existing_path_is_refused() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.rs"), "a").unwrap();
    fs::write(root.path().join("b.rs"), "b").unwrap();
    let error = move_path(&policy(root.path()), Path::new("a.rs"), Path::new("b.rs")).unwrap_err();
    assert!(error.to_string().contains("already exists"));
    // Neither file moved.
    assert_eq!(fs::read_to_string(root.path().join("b.rs")).unwrap(), "b");
}

/// A delete is the least reversible edit there is, so "the file I read" and
/// "the file on disk" being different matters more here than anywhere else.
#[test]
fn deleting_a_file_needs_its_current_hash() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("code.rs");
    fs::write(&path, "one").unwrap();
    let policy = policy(root.path());

    assert!(delete_path(&policy, Path::new("code.rs"), None, false).is_err());
    assert!(delete_path(&policy, Path::new("code.rs"), Some("stale"), false).is_err());
    assert!(path.exists(), "a refused delete removed the file anyway");

    let hash = read_file(&policy, Path::new("code.rs"))
        .unwrap()
        .artifact_hash;
    let removed = delete_path(&policy, Path::new("code.rs"), Some(&hash), false).unwrap();
    assert_eq!(removed.entries, 1);
    assert!(!path.exists());
}

/// A directory has no single hash, so removing one is deliberate rather than
/// guarded -- and the count of what went is returned so the audit says how
/// much rather than only that something did.
#[test]
fn a_directory_is_only_removed_when_that_was_asked_for() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("build/nested")).unwrap();
    fs::write(root.path().join("build/one.o"), "x").unwrap();
    fs::write(root.path().join("build/nested/two.o"), "x").unwrap();
    let policy = policy(root.path());

    assert!(delete_path(&policy, Path::new("build"), None, false).is_err());
    assert!(root.path().join("build").exists());

    let removed = delete_path(&policy, Path::new("build"), None, true).unwrap();
    assert!(removed.entries >= 2, "{removed:?}");
    assert!(!root.path().join("build").exists());
}

/// Both ends are resolved against the root, so neither reaches outside it.
#[test]
fn neither_end_of_a_move_may_leave_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.rs"), "a").unwrap();
    let policy = policy(root.path());
    assert!(move_path(&policy, Path::new("a.rs"), Path::new("../escaped.rs")).is_err());
    assert!(move_path(&policy, Path::new("../../etc/hosts"), Path::new("here.rs")).is_err());
    assert!(delete_path(&policy, Path::new("../outside.txt"), Some("x"), false).is_err());
    assert!(make_directory(&policy, Path::new("../outside")).is_err());
}

/// A symlink is refused rather than followed: what it points at may live
/// outside the workspace, and deleting through one is deleting there.
#[cfg(unix)]
#[test]
fn a_symlink_is_not_deleted_through() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("precious.txt");
    fs::write(&target, "keep me").unwrap();
    std::os::unix::fs::symlink(&target, root.path().join("link.txt")).unwrap();

    assert!(
        delete_path(
            &policy(root.path()),
            Path::new("link.txt"),
            Some("x"),
            false
        )
        .is_err()
    );
    assert!(target.exists(), "the target was removed through the link");
}

// ------------------------------------------------------------------- vcs

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(status.status.success(), "git {args:?} failed");
}

#[test]
fn status_and_diff_report_what_actually_changed() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    git(root, &["init", "-q", "-b", "main"]);
    fs::write(root.join("code.rs"), "fn one() {}\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "first"]);

    fs::write(root.join("code.rs"), "fn two() {}\n").unwrap();
    fs::write(root.join("added.rs"), "new\n").unwrap();

    let policy = policy(root);
    let status = block_on(vcs_status(&policy)).unwrap();
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert!(status.head.is_some(), "HEAD was not read");
    let changed: Vec<&str> = status
        .changed
        .iter()
        .map(|(_, path)| path.as_str())
        .collect();
    assert!(changed.contains(&"code.rs"), "{changed:?}");
    assert!(changed.contains(&"added.rs"), "{changed:?}");

    let diff = block_on(vcs_diff(&policy, &[])).unwrap();
    assert_eq!(diff.exit_code, Some(0));
    assert!(diff.stdout.contains("-fn one() {}"), "{}", diff.stdout);
    assert!(diff.stdout.contains("+fn two() {}"), "{}", diff.stdout);
}

/// A workspace that is not a checkout reports no branch rather than inventing
/// one, which is the rule `session show` already follows.
#[test]
fn a_workspace_outside_version_control_reports_no_branch() {
    let root = tempfile::tempdir().unwrap();
    let status = block_on(vcs_status(&policy(root.path())));
    assert!(
        status.is_err() || status.unwrap().branch.is_none(),
        "a non-checkout claimed a branch"
    );
}
