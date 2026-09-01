//! Malformed provider replies and non-deterministic verification.
//!
//! A model's output is untrusted input like any other. A reply that cannot be
//! parsed into exactly one typed action must be refused, never coerced into
//! something executable.

use poorai_orchestrator::parse_action_proposal;
use poorai_tools::{SandboxPolicy, ToolPolicy};
use poorai_verify::{
    FailureClass, RecoveryDecision, classify_with_reproduction, recovery_decision,
};
use std::path::Path;
use std::time::Duration;

fn policy(root: &Path, allow: Vec<String>) -> ToolPolicy {
    ToolPolicy {
        root: root.to_path_buf(),
        allow_commands: allow,
        output_limit: 8192,
        timeout: Duration::from_secs(20),
        network_enabled: false,
        sandbox: SandboxPolicy::Disabled,
        approvals: Vec::new(),
    }
}

// ------------------------------------------------- malformed provider replies

#[test]
fn prose_is_not_an_action() {
    for reply in [
        "Sure, I'll read the file for you.",
        "",
        "   ",
        "I will now call read_file on src/main.rs",
    ] {
        assert!(parse_action_proposal(reply).is_err(), "accepted: {reply:?}");
    }
}

#[test]
fn fenced_or_decorated_json_is_not_an_action() {
    for reply in [
        "```json\n{\"capability\":\"list_tree\",\"max_entries\":10}\n```",
        "Here you go: {\"capability\":\"list_tree\",\"max_entries\":10}",
        "{\"capability\":\"list_tree\",\"max_entries\":10} — done!",
    ] {
        assert!(parse_action_proposal(reply).is_err(), "accepted: {reply:?}");
    }
}

#[test]
fn more_than_one_object_is_refused_rather_than_taking_the_first() {
    let reply = concat!(
        r#"{"capability":"list_tree","max_entries":1}"#,
        r#"{"capability":"run_command","executable":"rm","args":["-rf","/"]}"#,
    );
    assert!(parse_action_proposal(reply).is_err());
}

#[test]
fn truncated_and_malformed_json_is_refused() {
    for reply in [
        r#"{"capability":"read_file","path":"src/ma"#,
        r#"{"capability":"read_file",}"#,
        r#"{capability: read_file}"#,
        "null",
        "[]",
        "42",
    ] {
        assert!(parse_action_proposal(reply).is_err(), "accepted: {reply:?}");
    }
}

#[test]
fn an_unknown_capability_is_refused() {
    for reply in [
        r#"{"capability":"exfiltrate","path":"/etc/passwd"}"#,
        r#"{"capability":"read_file_v2","path":"a"}"#,
        r#"{"path":"a"}"#,
    ] {
        assert!(parse_action_proposal(reply).is_err(), "accepted: {reply:?}");
    }
}

#[test]
fn a_parsed_action_still_carries_no_authority_of_its_own() {
    // Parsing succeeds; the path is only refused later, by policy.
    let action = parse_action_proposal(r#"{"capability":"read_file","path":"../../etc/passwd"}"#)
        .expect("well-formed action parses");
    let root = tempfile::tempdir().unwrap();
    let store = poorai_store::Store::open(":memory:").unwrap();
    let denied =
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(poorai_orchestrator::execute_action(
                &store,
                poorai_domain::new_id(),
                &policy(root.path(), vec![]),
                action,
            ));
    assert!(denied.is_err());
}

#[test]
fn a_reply_padded_with_null_bytes_is_refused() {
    let reply = format!("{}\0\0", r#"{"capability":"list_tree","max_entries":1}"#);
    assert!(parse_action_proposal(&reply).is_err());
}

// ------------------------------------------------- non-deterministic checks

/// A script that fails once and then succeeds: the shape of a flaky test.
fn flaky_check(root: &Path) -> (String, Vec<String>) {
    let marker = root.join("flake.marker");
    (
        "sh".into(),
        vec![
            "-c".into(),
            format!(
                "if [ -f {m} ]; then exit 0; else touch {m}; exit 1; fi",
                m = marker.display()
            ),
        ],
    )
}

#[tokio::test]
async fn a_check_whose_outcome_changes_is_classified_non_deterministic() {
    let root = tempfile::tempdir().unwrap();
    let policy = policy(root.path(), vec!["sh".into()]);
    let (command, args) = flaky_check(root.path());
    let first = poorai_tools::run_command(&policy, &command, &args)
        .await
        .unwrap();
    assert_eq!(first.exit_code, Some(1));
    let class = classify_with_reproduction(&policy, &command, &args, &first)
        .await
        .unwrap();
    assert!(matches!(class, FailureClass::NonDeterminism));
}

#[tokio::test]
async fn a_reproducible_failure_stays_classified_by_its_output() {
    let root = tempfile::tempdir().unwrap();
    let policy = policy(root.path(), vec!["sh".into()]);
    let args = vec![
        "-c".to_string(),
        "echo 'error: broken' >&2; exit 1".to_string(),
    ];
    let first = poorai_tools::run_command(&policy, "sh", &args)
        .await
        .unwrap();
    let class = classify_with_reproduction(&policy, "sh", &args, &first)
        .await
        .unwrap();
    assert!(matches!(class, FailureClass::Compilation));
}

#[tokio::test]
async fn a_passing_check_is_not_re_run() {
    let root = tempfile::tempdir().unwrap();
    let policy = policy(root.path(), vec!["sh".into()]);
    let counter = root.path().join("runs");
    let args = vec![
        "-c".to_string(),
        format!("echo x >> {}; exit 0", counter.display()),
    ];
    let first = poorai_tools::run_command(&policy, "sh", &args)
        .await
        .unwrap();
    let _ = classify_with_reproduction(&policy, "sh", &args, &first)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap().lines().count(),
        1
    );
}

/// The reason the classification matters: a flake must not authorise an edit.
#[test]
fn non_determinism_never_authorizes_an_edit() {
    let budget = poorai_verify::RecoveryBudget::default();
    assert!(matches!(
        recovery_decision(FailureClass::NonDeterminism, 0, 0, &budget),
        RecoveryDecision::Stop { .. }
    ));
    // Where an assertion failure, with budget left, would have.
    assert!(matches!(
        recovery_decision(FailureClass::Assertion, 0, 0, &budget),
        RecoveryDecision::EditAndRetry { .. }
    ));
}
