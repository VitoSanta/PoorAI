//! Stamps the build with the commit the harness was compiled from.
//!
//! A revision maintained by hand drifts: twenty-one commits changed this
//! harness while the constant still read v2, so every report claimed a version
//! that had not existed for a day. Deriving it means a report cannot describe
//! an agent other than the one that produced it.

fn main() {
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|out| !out.stdout.is_empty())
        .unwrap_or(false);
    // An uncommitted change is a different harness from the commit it sits on,
    // and a report that hides that is claiming a reproducibility it lacks.
    let suffix = if dirty { "-dirty" } else { "" };
    println!("cargo:rustc-env=POORAI_HARNESS_REV={commit}{suffix}");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
}
