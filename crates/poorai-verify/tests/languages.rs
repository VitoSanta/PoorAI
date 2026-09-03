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

// ------------------------------------------------ discovery beyond a list

/// A registry is still a list somebody chose. CI configuration is not a guess
/// about the project — it is the commands the project runs to check itself,
/// written by its authors, and it exists for languages nobody here has heard
/// of.
#[test]
fn a_project_is_verified_by_what_its_ci_says_even_in_an_unknown_language() {
    let root = project(&[
        ("src/main.zig", "pub fn main() void {}"),
        (
            ".github/workflows/ci.yml",
            "jobs:\n  build:\n    steps:\n      - run: zig build test\n",
        ),
    ]);
    let checks = discover_checks(root.path(), "targeted").unwrap();
    assert_eq!(
        checks,
        vec![("zig".to_string(), vec!["build".into(), "test".into()])]
    );
    // And its toolchain is permitted, or the check could not run.
    assert!(required_executables(root.path()).contains(&"zig".to_string()));
}

#[test]
fn other_ci_vendors_are_read_the_same_way() {
    for (path, body, executable) in [
        (".gitlab-ci.yml", "script:\n  - mix test\n", "mix"),
        (".circleci/config.yml", "      - run: sbt test\n", "sbt"),
        // A verb nobody here anticipated: common-test, not "test".
        (".travis.yml", "script:\n  - rebar3 ct\n", "rebar3"),
    ] {
        let root = project(&[(path, body)]);
        let checks = discover_checks(root.path(), "targeted").unwrap();
        assert_eq!(
            checks.first().map(|(e, _)| e.as_str()),
            Some(executable),
            "{path} did not yield {executable}"
        );
    }
}

/// A deploy step is not a verification step, and running one would do
/// something the repository never asked a checker to do.
#[test]
fn deployment_and_publication_steps_are_not_checks() {
    let root = project(&[(
        ".github/workflows/ci.yml",
        "steps:\n  - run: docker push registry/app\n  - run: npm publish\n  - run: ./deploy.sh test\n  - run: cargo test\n",
    )]);
    let checks = discover_checks(root.path(), "targeted").unwrap();
    assert_eq!(
        checks,
        vec![("cargo".to_string(), vec!["test".to_string()])]
    );
}

/// A step that chains or redirects is a script; running its first word through
/// the tool boundary would not mean what the file says.
#[test]
fn chained_and_templated_steps_are_skipped() {
    let root = project(&[(
        ".github/workflows/ci.yml",
        "steps:\n  - run: make setup && make test\n  - run: test ${{ matrix.os }}\n  - run: go test ./...\n",
    )]);
    let checks = discover_checks(root.path(), "targeted").unwrap();
    assert_eq!(
        checks,
        vec![("go".to_string(), vec!["test".into(), "./...".into()])]
    );
}

/// Ordered by how directly the source speaks for the repository: an explicit
/// declaration is the repository saying it, CI is the repository doing it, and
/// the registry is poorAI guessing from a file name.
#[test]
fn an_explicit_declaration_outranks_ci_which_outranks_the_registry() {
    let ci = ".github/workflows/ci.yml";
    let ci_body = "steps:\n  - run: just test\n";
    let with_all = project(&[
        ("Cargo.toml", "[package]"),
        (ci, ci_body),
        (
            ".poorai/checks.json",
            r#"{"checks":[{"executable":"nextest","args":["run"]}]}"#,
        ),
    ]);
    assert_eq!(
        discover_checks(with_all.path(), "targeted").unwrap()[0].0,
        "nextest"
    );
    let with_ci = project(&[("Cargo.toml", "[package]"), (ci, ci_body)]);
    assert_eq!(
        discover_checks(with_ci.path(), "targeted").unwrap()[0].0,
        "just"
    );
    let bare = project(&[("Cargo.toml", "[package]")]);
    assert_eq!(
        discover_checks(bare.path(), "targeted").unwrap()[0].0,
        "cargo"
    );
}

/// Denying `python` to a project whose declared check runs `python3` refuses
/// the interpreter it is already permitted to run. Measured: a run spent an
/// action on exactly that refusal.
#[test]
fn an_interpreter_is_not_denied_under_its_other_name() {
    // No build-system marker: the declaration is the only source of the
    // executable, so an expansion that runs before declarations are read
    // cannot pass this by reaching the marker registry instead.
    let root = project(&[(
        ".poorai/checks.json",
        r#"{"checks":[{"executable":"python3","args":["check.py"]}]}"#,
    )]);
    let executables = required_executables(root.path());
    for name in ["python3", "python"] {
        assert!(executables.contains(&name.to_string()), "{name} denied");
    }
    // The same for an executable that only CI names.
    let ci = project(&[(
        ".github/workflows/test.yml",
        "jobs:\n  test:\n    steps:\n      - run: python3 -m pytest\n",
    )]);
    let from_ci = required_executables(ci.path());
    assert!(
        from_ci.contains(&"python".to_string()),
        "CI-declared interpreter denied under its other name: {from_ci:?}"
    );

    // A JavaScript project gets npx alongside npm and node for the same reason.
    let js = project(&[("package.json", r#"{"scripts":{"test":"jest"}}"#)]);
    let js_executables = required_executables(js.path());
    for name in ["npm", "node", "npx"] {
        assert!(js_executables.contains(&name.to_string()), "{name} denied");
    }
}

/// A bare `- item` is a command only where the list it belongs to is a list of
/// commands. GitLab and Travis write `script:` followed by such a list; GitHub
/// writes `- uses: actions/checkout@v5`, which is a step that uses an action
/// and is not a command at all.
///
/// Measured: taking every list item produced a check named `uses:`, which
/// cannot execute and so failed on every turn of every run in that repository,
/// scoring a correct fix as a failure.
#[test]
fn a_github_step_that_uses_an_action_is_not_a_command() {
    let root = project(&[(
        ".github/workflows/main.yml",
        "name: CI\njobs:\n  test:\n    steps:\n      - uses: actions/checkout@v5\n      \
         - uses: actions/setup-node@v4\n        with:\n          node-version: 20\n      \
         - run: npm test\n",
    )]);
    let checks = discover_checks(root.path(), "targeted").unwrap();
    assert!(
        !checks
            .iter()
            .any(|(executable, _)| executable.contains(':')),
        "a YAML key was taken for an executable: {checks:?}"
    );
    assert!(
        checks
            .iter()
            .any(|(executable, args)| executable == "npm" && args == &["test".to_string()]),
        "the one real command was lost: {checks:?}"
    );
}

/// The bare-list form still works where the list really is a list of commands.
#[test]
fn a_gitlab_script_list_is_still_read() {
    let root = project(&[(
        ".gitlab-ci.yml",
        "test:\n  script:\n    - cargo test\n    - cargo clippy\n",
    )]);
    let checks = discover_checks(root.path(), "targeted").unwrap();
    assert!(
        checks
            .iter()
            .any(|(e, a)| e == "cargo" && a == &["test".to_string()]),
        "{checks:?}"
    );
}

/// A discovered check is not filtered by whether its executable exists on this
/// machine. Two fixtures caught an attempt to do that: `mix`, `sbt` and
/// `rebar3` are absent here, so the filter silently discarded the Elixir, Scala
/// and Erlang cases. Discovery that depends on what happens to be installed
/// gives different answers on different machines, and it contradicts
/// provisioning, where the toolchain is precisely what is not there yet.
///
/// A check that cannot run is handled where it can be handled honestly: the
/// deployment is told which checks were already failing when the run opened.
#[test]
fn discovery_does_not_depend_on_what_is_installed_here() {
    let root = project(&[(
        ".github/workflows/main.yml",
        "jobs:\n  test:\n    steps:\n      - run: mix test\n",
    )]);
    let checks = discover_checks(root.path(), "targeted").unwrap();
    assert_eq!(
        checks.first().map(|(e, _)| e.as_str()),
        Some("mix"),
        "a check was dropped for not being installed on this machine: {checks:?}"
    );
}
