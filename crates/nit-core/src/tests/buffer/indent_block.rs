//! Block indent / dedent (T5) buffer tests. Pin the multi-line
//! invariants of `Buffer::indent_selection` and `Buffer::dedent_selection`:
//! every selected line shifts by one unit, a partially-indented line
//! shrinks to flush-left without panic, and the whole block rewinds in a
//! single undo step.

use crate::buffer::Buffer;

fn buf(text: &str) -> Buffer {
    Buffer::from_str("t", text, None)
}

/// Build a 3-line selection anchored at the head of `(start_line, 0)` with
/// the cursor at `(end_line, end_col)`. Mirrors the shape produced by a
/// `V` + motion sequence in visual mode.
fn select_lines(buffer: &mut Buffer, start_line: usize, end_line: usize, end_col: usize) {
    buffer.cursor.line = start_line;
    buffer.cursor.col = 0;
    buffer.set_selection_anchor();
    buffer.cursor.line = end_line;
    buffer.cursor.col = end_col;
}

#[test]
fn indent_selection_prepends_unit_to_each_line() {
    let mut buffer = buf("fn foo() {\n    one\n    two\n}\n");
    // Default unit is 4 spaces (inferred from "    one"). Select lines 1-2.
    select_lines(&mut buffer, 1, 2, 3);
    assert!(buffer.indent_selection());
    let lines: Vec<String> = buffer
        .content_as_string()
        .lines()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(lines[0], "fn foo() {");
    assert_eq!(lines[1], "        one");
    assert_eq!(lines[2], "        two");
    assert_eq!(lines[3], "}");
}

#[test]
fn indent_selection_single_undo_rewinds_block() {
    let original = "a\nb\nc\n    seed\n";
    let mut buffer = buf(original);
    select_lines(&mut buffer, 0, 2, 1);
    assert!(buffer.indent_selection());
    let after = buffer.content_as_string();
    assert_ne!(after, original);
    assert!(buffer.undo());
    assert_eq!(buffer.content_as_string(), original);
}

#[test]
fn dedent_selection_strips_one_unit_per_line() {
    let mut buffer = buf("    one\n    two\n    three\n");
    select_lines(&mut buffer, 0, 2, 5);
    assert!(buffer.dedent_selection());
    let lines: Vec<String> = buffer
        .content_as_string()
        .lines()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(lines[0], "one");
    assert_eq!(lines[1], "two");
    assert_eq!(lines[2], "three");
}

#[test]
fn dedent_strips_partial_indent_without_panic() {
    // Unit inferred as 4 spaces but line 1 only has 2 — vim's `<<` removes
    // those 2 and leaves the line flush-left rather than panicking.
    let mut buffer = buf("    seed\n  two\n");
    select_lines(&mut buffer, 1, 1, 4);
    assert!(buffer.dedent_selection());
    let lines: Vec<String> = buffer
        .content_as_string()
        .lines()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(lines[1], "two");
}

#[test]
fn dedent_on_zero_indent_lines_is_noop() {
    let mut buffer = buf("alpha\nbeta\n");
    select_lines(&mut buffer, 0, 1, 3);
    assert!(!buffer.dedent_selection());
    assert_eq!(buffer.content_as_string(), "alpha\nbeta\n");
}

#[test]
fn dedent_selection_single_undo_rewinds_block() {
    let original = "    one\n    two\n    three\n";
    let mut buffer = buf(original);
    select_lines(&mut buffer, 0, 2, 5);
    assert!(buffer.dedent_selection());
    assert_ne!(buffer.content_as_string(), original);
    assert!(buffer.undo());
    assert_eq!(buffer.content_as_string(), original);
}

#[test]
fn indent_with_no_selection_indents_current_line() {
    let mut buffer = buf("alpha\nbeta\n");
    buffer.cursor.line = 1;
    buffer.cursor.col = 2;
    assert!(buffer.indent_selection());
    let lines: Vec<String> = buffer
        .content_as_string()
        .lines()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(lines[0], "alpha");
    assert_eq!(lines[1], "    beta");
}

#[test]
fn indent_shifts_cursor_and_anchor_within_logical_text() {
    // Cursor and anchor both land on the same line; after a 4-space indent
    // their relative offset to the line's *content* must be preserved.
    let mut buffer = buf("alpha\nbeta\n");
    buffer.cursor.line = 1;
    buffer.cursor.col = 0;
    buffer.set_selection_anchor();
    buffer.cursor.col = 3;
    assert!(buffer.indent_selection());
    // Indent ends visual mode in the action layer; here we just verify the
    // buffer-level anchor + cursor moved with the inserted prefix.
    assert_eq!(buffer.cursor.col, 7);
}

#[test]
fn indent_uses_tab_when_buffer_is_tab_indented() {
    let mut buffer = buf("\tone\ntwo\n");
    select_lines(&mut buffer, 1, 1, 1);
    assert!(buffer.indent_selection());
    let lines: Vec<String> = buffer
        .content_as_string()
        .lines()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(lines[1], "\ttwo");
}

#[test]
fn dedent_strips_one_tab_on_tab_indented_buffer() {
    let mut buffer = buf("\tone\n\t\ttwo\n");
    select_lines(&mut buffer, 0, 1, 2);
    assert!(buffer.dedent_selection());
    let lines: Vec<String> = buffer
        .content_as_string()
        .lines()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(lines[0], "one");
    assert_eq!(lines[1], "\ttwo");
}

#[test]
fn indent_then_dedent_round_trips_to_original() {
    let original = "alpha\nbeta\n    gamma\n";
    let mut buffer = buf(original);
    select_lines(&mut buffer, 0, 1, 3);
    assert!(buffer.indent_selection());
    select_lines(&mut buffer, 0, 1, 7);
    assert!(buffer.dedent_selection());
    assert_eq!(buffer.content_as_string(), original);
}
