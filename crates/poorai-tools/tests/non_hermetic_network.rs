//! Non-hermetic: these reach the real network and a real package registry.
//!
//! They are `#[ignore]`d so the default suite stays hermetic, and run
//! explicitly with `cargo test -p poorai-tools -- --ignored`. Their subject is
//! the boundary itself, which cannot be verified against a fake.

use poorai_tools::{Approval, SandboxPolicy, ToolPolicy, run_command};
use std::path::Path;
use std::time::Duration;

fn policy(root: &Path, network: bool) -> ToolPolicy {
    ToolPolicy {
        root: root.to_path_buf(),
        allow_commands: vec!["npm".into(), "cargo".into()],
        output_limit: 64 * 1024,
        timeout: Duration::from_secs(300),
        sandbox: SandboxPolicy::Preferred,
        approvals: if network {
            vec![Approval::NetworkAccess]
        } else {
            vec![]
        },
    }
}

fn install(root: &Path, network: bool) -> poorai_tools::ToolResult {
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"probe","version":"1.0.0","dependencies":{"lodash.isempty":"^4.4.0"}}"#,
    )
    .unwrap();
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(run_command(
            &policy(root, network),
            "npm",
            &[
                "install".to_string(),
                "--no-audit".into(),
                "--no-fund".into(),
            ],
        ))
        .unwrap()
}

/// Dependency resolution is the reason the grant exists. Without it the same
/// command must fail, in a workspace with no cache a previous run left behind.
#[test]
#[ignore = "reaches the network and a package registry"]
fn dependencies_install_under_a_grant_and_not_without_one() {
    let granted = tempfile::tempdir().unwrap();
    let result = install(granted.path(), true);
    assert!(result.sandboxed);
    assert_eq!(
        result.exit_code,
        Some(0),
        "stderr: {}",
        &result.stderr[..result.stderr.len().min(500)]
    );
    assert!(granted.path().join("node_modules/lodash.isempty").exists());

    let ungranted = tempfile::tempdir().unwrap();
    let refused = install(ungranted.path(), false);
    assert_ne!(refused.exit_code, Some(0), "installed with no grant");
    assert!(
        !ungranted
            .path()
            .join("node_modules/lodash.isempty")
            .exists()
    );
}

/// HOME points into the workspace so package managers keep their caches inside
/// the boundary. A toolchain that needs HOME for anything else must still work.
#[test]
#[ignore = "runs a real cargo build"]
fn cargo_builds_with_home_inside_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname=\"t\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("src/lib.rs"),
        "pub fn v() -> i32 { 1 }\n#[cfg(test)]\nmod t {\n    #[test]\n    fn w() { assert_eq!(super::v(), 1); }\n}\n",
    )
    .unwrap();
    let result = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(run_command(
            &policy(root.path(), false),
            "cargo",
            &["test".to_string(), "--quiet".into()],
        ))
        .unwrap();
    assert_eq!(result.exit_code, Some(0), "stderr: {}", result.stderr);
}
