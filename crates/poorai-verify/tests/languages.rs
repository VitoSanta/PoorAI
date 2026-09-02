//! Verification appropriate to the repository, whatever the repository is.

use poorai_verify::{discover_checks, required_executables};
use std::fs;

fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (path, contents) in files {
        let full = root.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, contents).unwrap();
    }
    root
}

#[test]
fn each_recognised_build_system_yields_its_own_check() {
    let cases: &[(&str, &str, &str)] = &[
        ("Cargo.toml", "[package]", "cargo"),
        ("go.mod", "module x", "go"),
        ("pom.xml", "<project/>", "mvn"),
        ("build.gradle", "", "gradle"),
        ("Package.swift", "", "swift"),
        ("pubspec.yaml", "name: x", "flutter"),
        ("mix.exs", "", "mix"),
        ("pyproject.toml", "[project]", "pytest"),
        ("requirements.txt", "", "pytest"),
        ("Gemfile", "", "bundle"),
        ("composer.json", "{}", "composer"),
        ("Makefile", "test:\n\t@true\n", "make"),
        ("CMakeLists.txt", "", "ctest"),
        ("global.json", "{}", "dotnet"),
    ];
    for (marker, contents, executable) in cases {
        let root = project(&[(marker, contents)]);
        let checks = discover_checks(root.path(), "targeted").unwrap();
        assert_eq!(
            checks.first().map(|(e, _)| e.as_str()),
            Some(*executable),
            "{marker} did not resolve to {executable}"
        );
        // A project whose own toolchain is denied cannot be verified.
        assert!(required_executables(root.path()).contains(&executable.to_string()));
    }
}

/// The escape hatch that stops the registry being a closed world.
#[test]
fn a_repository_may_declare_its_own_checks() {
    let root = project(&[
        ("Cargo.toml", "[package]"),
        (
            ".poorai/checks.json",
            r#"{"checks":[{"executable":"just","args":["verify"]}]}"#,
        ),
    ]);
    let checks = discover_checks(root.path(), "targeted").unwrap();
    // The repository knows how it is verified; the registry only guesses.
    assert_eq!(
        checks,
        vec![("just".to_string(), vec!["verify".to_string()])]
    );
    assert!(required_executables(root.path()).contains(&"just".to_string()));
}

/// An unrecognised repository is not silently treated as passing: it yields no
/// checks, and a run against it reports that it verified nothing.
#[test]
fn an_unrecognised_repository_yields_no_checks_rather_than_a_wrong_one() {
    let root = project(&[("main.cbl", "IDENTIFICATION DIVISION.")]);
    assert!(discover_checks(root.path(), "targeted").unwrap().is_empty());
    assert!(required_executables(root.path()).is_empty());
}

/// The regression this exists to prevent: before this registry, everything but
/// Rust and JavaScript fell into the unrecognised case.
#[test]
fn a_python_project_is_verifiable() {
    let root = project(&[("pyproject.toml", "[project]\nname = 'x'\n")]);
    assert!(!discover_checks(root.path(), "targeted").unwrap().is_empty());
}

#[test]
fn a_javascript_project_needs_its_runtime_as_well_as_its_runner() {
    let root = project(&[("package.json", r#"{"scripts":{"test":"jest"}}"#)]);
    let executables = required_executables(root.path());
    assert!(executables.contains(&"npm".to_string()));
    assert!(executables.contains(&"node".to_string()));
}

/// A polyglot repository needs every toolchain it contains, not the first.
#[test]
fn a_polyglot_repository_gets_every_toolchain_it_needs() {
    let root = project(&[
        ("Cargo.toml", "[package]"),
        ("pyproject.toml", "[project]"),
        ("go.mod", "module x"),
    ]);
    let executables = required_executables(root.path());
    for expected in ["cargo", "pytest", "go"] {
        assert!(executables.contains(&expected.to_string()), "{expected}");
    }
}
