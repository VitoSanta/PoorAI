//! A prompt was strings glued together at the call site.
//!
//! The repository excerpts and the task shared one user message, so nothing
//! downstream could tell them apart; and the budget was a fraction, so a
//! prompt that was too large could only be made smaller by guessing which part
//! to cut.

use poorai_orchestrator::context::{Section, SectionKind, compile, estimate_tokens};

fn filler(chars: usize) -> String {
    "x".repeat(chars)
}

#[test]
fn every_section_carries_its_own_cost_and_hash() {
    let (messages, compiled) = compile(
        vec![
            Section::new(SectionKind::System, "you are an agent"),
            Section::new(SectionKind::RepositoryExcerpts, "--- src/a.rs\nfn a() {}"),
            Section::new(SectionKind::Task, "fix the parser"),
        ],
        8_192,
    );
    assert_eq!(compiled.sections.len(), 3);
    for section in &compiled.sections {
        assert!(section.estimated_tokens > 0, "{section:?}");
        assert!(!section.content_hash.is_empty());
        assert!(!section.dropped);
    }
    // The task is its own message, not glued to the excerpts.
    assert!(
        messages.iter().any(|m| m.content == "fix the parser"),
        "{messages:?}"
    );
    assert!(!compiled.reduced);
}

/// A prompt that fills the context leaves the deployment nowhere to answer,
/// which is a failure that looks like a refusal.
#[test]
fn output_headroom_is_reserved_before_anything_is_fitted() {
    let (_, compiled) = compile(vec![Section::new(SectionKind::Task, "fix it")], 1_000);
    assert!(compiled.reserve_tokens > 0);
    assert!(compiled.reserve_tokens < 1_000);
}

/// Excerpts before the ledger: excerpts are a starting point the agent can
/// rebuild with search and read_file, while the ledger is the only account of
/// what earlier runs did and cannot be recovered from the workspace.
#[test]
fn excerpts_are_given_up_before_the_ledger_is() {
    let (_, compiled) = compile(
        vec![
            Section::new(SectionKind::System, filler(400)),
            Section::new(SectionKind::SessionLedger, filler(8_000)),
            Section::new(SectionKind::RepositoryExcerpts, filler(40_000)),
            Section::new(SectionKind::Task, "fix the parser"),
        ],
        4_096,
    );
    let of = |kind: SectionKind| {
        compiled
            .sections
            .iter()
            .find(|section| section.kind == kind)
            .unwrap()
            .clone()
    };
    let excerpts = of(SectionKind::RepositoryExcerpts);
    let ledger = of(SectionKind::SessionLedger);
    assert!(
        excerpts.dropped || excerpts.truncated_to.is_some(),
        "the excerpts were kept whole while the prompt did not fit"
    );
    assert!(!ledger.dropped, "the ledger was given up first");
    assert!(ledger.truncated_to.is_none(), "the ledger was cut first");
    assert!(compiled.reduced, "a reduction was not recorded");
}

/// A run without its goal is not a cheaper run, it is a different one.
#[test]
fn the_task_and_the_system_prompt_are_never_cut() {
    let (messages, compiled) = compile(
        vec![
            Section::new(SectionKind::System, filler(20_000)),
            Section::new(SectionKind::RepositoryExcerpts, filler(80_000)),
            Section::new(SectionKind::Task, "fix the parser"),
        ],
        512,
    );
    let task = compiled
        .sections
        .iter()
        .find(|section| section.kind == SectionKind::Task)
        .unwrap();
    assert!(!task.dropped);
    assert!(task.truncated_to.is_none());
    assert!(
        messages
            .iter()
            .any(|m| m.content.contains("fix the parser"))
    );
    // The prompt still does not fit, and says so rather than pretending.
    assert!(compiled.estimated_tokens > compiled.context_tokens as usize);
}

/// A section cut to a few hundred characters is not a smaller section, it is a
/// misleading one: half an excerpt reads like a whole file.
#[test]
fn a_section_is_dropped_rather_than_cut_to_a_useless_stub() {
    let (_, compiled) = compile(
        vec![
            Section::new(SectionKind::System, filler(4_000)),
            Section::new(SectionKind::RepositoryExcerpts, filler(60_000)),
            Section::new(SectionKind::Task, "fix it"),
        ],
        1_400,
    );
    let excerpts = compiled
        .sections
        .iter()
        .find(|section| section.kind == SectionKind::RepositoryExcerpts)
        .unwrap();
    assert!(excerpts.dropped, "{excerpts:?}");
}

/// Cutting a string mid-code-point panics, and a prompt that panics on a
/// non-ASCII repository is worse than one that keeps four bytes fewer.
#[test]
fn truncation_does_not_split_a_character() {
    let text: String = "é".repeat(30_000);
    let (_, compiled) = compile(
        vec![
            Section::new(SectionKind::System, "system"),
            Section::new(SectionKind::RepositoryExcerpts, text),
            Section::new(SectionKind::Task, "fix it"),
        ],
        4_096,
    );
    let excerpts = compiled
        .sections
        .iter()
        .find(|section| section.kind == SectionKind::RepositoryExcerpts)
        .unwrap();
    // Either kept whole, truncated on a boundary, or dropped -- never a panic.
    assert!(excerpts.dropped || excerpts.bytes > 0);
}

/// A backend that expects a single system message gets one, without the
/// sections losing their separate accounting. User sections are never merged:
/// gluing the excerpts to the task is what this file exists to undo.
#[test]
fn the_system_prompt_and_its_suffix_become_one_message() {
    let (messages, compiled) = compile(
        vec![
            Section::new(SectionKind::System, "base."),
            Section::new(SectionKind::ModelSuffix, "and this."),
            Section::new(SectionKind::Task, "fix it"),
        ],
        8_192,
    );
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert_eq!(messages[0].content, "base.and this.");
    // And the task is not glued to anything.
    assert_eq!(messages[1].content, "fix it");
    assert_eq!(compiled.sections.len(), 3, "accounting stayed per section");
}

#[test]
fn an_empty_section_is_not_carried() {
    let (messages, compiled) = compile(
        vec![
            Section::new(SectionKind::System, "system"),
            Section::new(SectionKind::SessionLedger, ""),
            Section::new(SectionKind::Task, "fix it"),
        ],
        8_192,
    );
    assert_eq!(compiled.sections.len(), 2);
    assert_eq!(messages.len(), 2);
}

#[test]
fn the_estimate_is_characters_over_four() {
    assert_eq!(estimate_tokens("abcd"), 1);
    assert_eq!(estimate_tokens("abcde"), 2);
    assert_eq!(estimate_tokens(""), 0);
}
