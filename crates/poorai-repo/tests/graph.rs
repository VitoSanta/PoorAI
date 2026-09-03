//! The index is incremental, and it has edges.
//!
//! Every run walked and re-read the whole repository, and retrieval then
//! re-read every file to score it before re-opening the ones it chose --
//! O(repository bytes) twice per run, on a workspace the previous run had
//! already read.

use poorai_repo::{extract_imports, index_incremental, retrieve, test_subject};
use std::fs;

#[test]
fn an_unchanged_file_is_not_read_again() {
    let root = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    for i in 0..5 {
        fs::write(
            root.path().join(format!("f{i}.rs")),
            format!("fn f{i}() {{}}"),
        )
        .unwrap();
    }

    let (_, first) = index_incremental(root.path(), Some(state.path())).unwrap();
    assert_eq!(first.files, 5);
    assert_eq!(first.read, 5, "the first pass has nothing to reuse");

    let (_, second) = index_incremental(root.path(), Some(state.path())).unwrap();
    assert_eq!(second.reused, 5, "an unchanged repository was read again");
    assert_eq!(second.read, 0);

    // One file changes, and only that one is read.
    fs::write(root.path().join("f2.rs"), "fn f2() { changed() }").unwrap();
    let (index, third) = index_incremental(root.path(), Some(state.path())).unwrap();
    assert_eq!(third.read, 1, "more than the changed file was read");
    assert_eq!(third.reused, 4);
    // And the record it carries is the new one, not the cached one.
    let changed = index
        .files
        .iter()
        .find(|file| file.path == "f2.rs")
        .unwrap();
    assert!(changed.terms.contains(&"changed".to_string()));
}

/// A deleted file that stays in the cache is a file retrieval can still rank,
/// which is worse than a slow index.
#[test]
fn a_deleted_file_is_forgotten() {
    let root = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    fs::write(root.path().join("kept.rs"), "fn kept() {}").unwrap();
    fs::write(root.path().join("gone.rs"), "fn gone() {}").unwrap();
    index_incremental(root.path(), Some(state.path())).unwrap();

    fs::remove_file(root.path().join("gone.rs")).unwrap();
    let (index, work) = index_incremental(root.path(), Some(state.path())).unwrap();
    assert_eq!(work.forgotten, 1);
    assert!(!index.files.iter().any(|file| file.path == "gone.rs"));
}

#[test]
fn imports_are_read_as_written_across_languages() {
    // The path as written, not trimmed to a module: the edge is matched by
    // substring, so keeping the whole path can only help it find the file and
    // trimming it guesses at where the module boundary is.
    assert_eq!(
        extract_imports("use crate::parser::Token;"),
        vec!["crate::parser::Token"]
    );
    assert_eq!(
        extract_imports("from app.parser import Token"),
        vec!["app.parser"]
    );
    assert_eq!(
        extract_imports("import java.util.List;"),
        vec!["java.util.List"]
    );
    assert_eq!(
        extract_imports("import { Token } from \"./parser\""),
        vec!["./parser"]
    );
    assert_eq!(extract_imports("#include \"parser.h\""), vec!["parser.h"]);
    // A comment is not an import.
    assert!(extract_imports("// use crate::parser;").is_empty());
}

#[test]
fn a_test_file_names_its_subject_by_convention() {
    assert_eq!(test_subject("test_parser.py").as_deref(), Some("parser"));
    assert_eq!(test_subject("parser_test.go").as_deref(), Some("parser"));
    assert_eq!(test_subject("parser.test.ts").as_deref(), Some("parser"));
    assert_eq!(test_subject("tests/parser.rs").as_deref(), Some("parser"));
    // An ordinary source file claims to test nothing.
    assert_eq!(test_subject("src/parser.rs"), None);
    // And a barrel file names no subject worth ranking.
    assert_eq!(test_subject("tests/mod.rs"), None);
}

/// A file the best candidate imports is about the task even when it never
/// names it. This is the edge `repository-intelligence.md` has specified since
/// the beginning and the ranking never had.
#[test]
fn a_neighbour_is_retrieved_even_when_it_never_names_the_task() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("src/handler.rs"),
        "use crate::tokenizer;\nfn handle_token_stream() { tokenizer::next() }\n",
    )
    .unwrap();
    // Never says "token": only the import reaches it.
    fs::write(
        root.path().join("src/tokenizer.rs"),
        "fn next() -> u8 { 0 }\nfn advance() {}\n",
    )
    .unwrap();
    fs::write(root.path().join("src/unrelated.rs"), "fn nothing() {}\n").unwrap();

    let (index, _) = index_incremental(root.path(), None).unwrap();
    let excerpts = retrieve(root.path(), &index, "token stream", 5, 4_000).unwrap();
    let paths: Vec<&str> = excerpts.iter().map(|e| e.path.as_str()).collect();
    assert!(paths.contains(&"src/handler.rs"), "{paths:?}");
    assert!(
        paths.contains(&"src/tokenizer.rs"),
        "the imported neighbour was not reached: {paths:?}"
    );
    let neighbour = excerpts
        .iter()
        .find(|e| e.path == "src/tokenizer.rs")
        .unwrap();
    // Why it was chosen is in the record, as it is for every other excerpt.
    assert!(
        neighbour.rationale.contains("imported by"),
        "{}",
        neighbour.rationale
    );
}

/// Proximity is evidence about the neighbourhood, not about the file, so it
/// must never outrank a file that actually defines what was asked for.
#[test]
fn proximity_never_outranks_a_direct_match() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("caller.rs"),
        "use crate::neighbour;\nfn calls_parse_document() {}\n",
    )
    .unwrap();
    fs::write(root.path().join("neighbour.rs"), "fn unrelated() {}\n").unwrap();
    fs::write(
        root.path().join("parse_document.rs"),
        "fn parse_document() { }\n",
    )
    .unwrap();

    let (index, _) = index_incremental(root.path(), None).unwrap();
    let excerpts = retrieve(root.path(), &index, "parse_document", 5, 4_000).unwrap();
    assert_eq!(
        excerpts.first().map(|e| e.path.as_str()),
        Some("parse_document.rs"),
        "{:?}",
        excerpts.iter().map(|e| &e.path).collect::<Vec<_>>()
    );
}
