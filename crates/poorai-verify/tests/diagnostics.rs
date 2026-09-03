//! A failure located in the source, rather than described in prose.
//!
//! Compiler and test output reached the deployment as bounded text, so recovery
//! aimed at a paragraph. Finding the file and line was work the model paid
//! actions for, and it is mechanical -- which is the harness's job.

use poorai_verify::diagnostics;

#[test]
fn rustc_output_yields_a_file_and_a_line() {
    let output = "error[E0308]: mismatched types\n  --> src/parser.rs:42:17\n   |\n42 |     let x: u32 = \"s\";\n";
    let found = diagnostics(output);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].path, "src/parser.rs");
    assert_eq!(found[0].line, Some(42));
    assert_eq!(found[0].column, Some(17));
}

#[test]
fn gcc_and_typescript_shapes_are_read_the_same_way() {
    let found = diagnostics(
        "src/main.c:12:5: error: expected ';' before '}' token\nsrc/app.ts:3:1: error: Cannot find name 'foo'.\n",
    );
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[0].path, "src/main.c");
    assert_eq!(found[0].line, Some(12));
    assert!(found[0].message.contains("expected ';'"));
    assert_eq!(found[1].path, "src/app.ts");
}

#[test]
fn a_python_traceback_yields_its_frames() {
    let output = "Traceback (most recent call last):\n  File \"app/handler.py\", line 88, in dispatch\n    return route(request)\nValueError: no route\n";
    let found = diagnostics(output);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].path, "app/handler.py");
    assert_eq!(found[0].line, Some(88));
}

/// A wrong location is worse than none: it sends the agent to edit a file that
/// is fine. So a line that does not clearly carry a path and a position is not
/// guessed at.
#[test]
fn prose_is_not_mined_for_locations() {
    let output = "\
running 12 tests
note: this is a message with a colon: and another
warning: 3 warnings emitted
Compiling poorai-verify v0.1.0
finished in 4.20s
http://example.com:8080/path
";
    let found = diagnostics(output);
    assert!(found.is_empty(), "invented locations: {found:?}");
}

#[test]
fn the_same_location_is_not_reported_twice() {
    let output = "  --> src/lib.rs:9:1\n  --> src/lib.rs:9:1\n";
    assert_eq!(diagnostics(output).len(), 1);
}

/// Bounded like every other output this project reads: a test suite that fails
/// everywhere must not turn into an unbounded list.
#[test]
fn the_number_of_diagnostics_is_bounded() {
    let output: String = (0..500)
        .map(|i| format!("  --> src/file{i}.rs:{i}:1\n"))
        .collect();
    assert!(diagnostics(&output).len() <= 32);
}
