use super::*;

#[test]
fn undo_single_step_on_selection_replace_char() {
    let mut buf = Buffer::from_str("test", "hello", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.set_selection_anchor();
    buf.move_end();

    buf.insert_char('x');
    assert_eq!(buf.content_as_string(), "x");

    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "hello");

    assert!(buf.redo());
    assert_eq!(buf.content_as_string(), "x");
}

#[test]
fn undo_single_step_on_selection_replace_str() {
    let mut buf = Buffer::from_str("test", "hello world", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.set_selection_anchor();
    buf.move_end();

    buf.insert_str("XYZ");
    assert_eq!(buf.content_as_string(), "XYZ");

    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "hello world");
}

#[test]
fn undo_group_breaks_on_cursor_move() {
    let mut buf = Buffer::empty("test", None);

    buf.insert_char('a');
    buf.move_left();
    buf.move_right();
    buf.insert_char('b');
    assert_eq!(buf.content_as_string(), "ab");

    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "a");

    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "");
}

#[test]
fn break_undo_group_splits_inserts() {
    let mut buf = Buffer::empty("test", None);

    buf.insert_char('a');
    buf.break_undo_group();
    buf.insert_str("b");
    assert_eq!(buf.content_as_string(), "ab");

    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "a");
}

#[test]
fn undo_single_step_on_selection_replace_newline_preserves_indent() {
    let mut buf = Buffer::from_str("test", "    foo", None);
    buf.cursor.line = 0;
    buf.cursor.col = 4;
    buf.set_selection_anchor();
    buf.move_end();

    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "    \n    ");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);

    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "    foo");
}

// --- Smart indent tests ---

#[test]
fn newline_after_open_brace_increases_indent() {
    let mut buf = Buffer::from_str("test", "fn main() {", None);
    buf.cursor.line = 0;
    buf.cursor.col = 11; // after '{'
    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "fn main() {\n    ");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn newline_after_open_paren_increases_indent() {
    let mut buf = Buffer::from_str("test", "foo(", None);
    buf.cursor.line = 0;
    buf.cursor.col = 4;
    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "foo(\n    ");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn newline_after_open_bracket_increases_indent() {
    let mut buf = Buffer::from_str("test", "let a = [", None);
    buf.cursor.line = 0;
    buf.cursor.col = 9;
    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "let a = [\n    ");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn newline_preserves_existing_indent_plus_extra() {
    let mut buf = Buffer::from_str("test", "    if x {", None);
    buf.cursor.line = 0;
    buf.cursor.col = 10; // after '{'
    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "    if x {\n        ");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 8);
}

#[test]
fn newline_without_opener_copies_indent_only() {
    let mut buf = Buffer::from_str("test", "    let x = 1;", None);
    buf.cursor.line = 0;
    buf.cursor.col = 14;
    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "    let x = 1;\n    ");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn newline_bracket_pair_expansion() {
    let mut buf = Buffer::from_str("test", "fn main() {}", None);
    buf.cursor.line = 0;
    buf.cursor.col = 11; // between '{' and '}'
    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "fn main() {\n    \n}");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn newline_bracket_pair_with_existing_indent() {
    let mut buf = Buffer::from_str("test", "    fn foo() {}", None);
    buf.cursor.line = 0;
    buf.cursor.col = 14; // between '{' and '}'
    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "    fn foo() {\n        \n    }");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 8);
}

#[test]
fn open_line_below_after_brace_increases_indent() {
    let mut buf = Buffer::from_str("test", "fn main() {\n}", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.open_line_below();
    assert_eq!(buf.content_as_string(), "fn main() {\n    \n}");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn open_line_below_after_colon_increases_indent() {
    let mut buf = Buffer::from_str("test", "def foo():", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.open_line_below();
    assert_eq!(buf.content_as_string(), "def foo():\n    ");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn open_line_below_no_opener_copies_indent() {
    let mut buf = Buffer::from_str("test", "    let x = 1;", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.open_line_below();
    assert_eq!(buf.content_as_string(), "    let x = 1;\n    ");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn indent_unit_detects_two_space_indent() {
    let buf = Buffer::from_str("test", "if true:\n  foo\n  bar\n", None);
    assert_eq!(buf.indent_unit(), "  ");
}

#[test]
fn indent_unit_detects_four_space_indent() {
    let buf = Buffer::from_str("test", "fn main() {\n    let x = 1;\n}\n", None);
    assert_eq!(buf.indent_unit(), "    ");
}

#[test]
fn indent_unit_detects_tab_indent() {
    let buf = Buffer::from_str("test", "fn main() {\n\tlet x = 1;\n}\n", None);
    assert_eq!(buf.indent_unit(), "\t");
}

#[test]
fn indent_unit_defaults_to_four_spaces() {
    let buf = Buffer::from_str("test", "no indentation here", None);
    assert_eq!(buf.indent_unit(), "    ");
}

#[test]
fn diff_detects_added_lines() {
    let mut buf = Buffer::from_str("test", "a\nb\nc\n", None);
    // No changes yet - all unchanged
    buf.compute_diff_if_needed();
    assert_eq!(buf.line_diff_status(0), LineDiffStatus::Unchanged);
    assert_eq!(buf.line_diff_status(1), LineDiffStatus::Unchanged);

    // Insert a new line
    buf.cursor.line = 1;
    buf.cursor.col = 0;
    buf.open_line_above();
    buf.insert_str("new");
    buf.compute_diff_if_needed();
    // The new line should be marked as Added
    assert_eq!(buf.line_diff_status(1), LineDiffStatus::Added);
}

#[test]
fn diff_detects_modified_lines() {
    let mut buf = Buffer::from_str("test", "hello\nworld\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 5;
    buf.insert_str(" there");
    buf.compute_diff_if_needed();
    assert_eq!(buf.line_diff_status(0), LineDiffStatus::Modified);
    assert_eq!(buf.line_diff_status(1), LineDiffStatus::Unchanged);
}

#[test]
fn diff_all_unchanged_on_open() {
    let buf = Buffer::from_str("test", "a\nb\nc\n", None);
    assert_eq!(buf.diff_statuses().len(), 0); // not computed yet
}

#[test]
fn diff_resets_on_mark_clean() {
    let mut buf = Buffer::from_str("test", "a\nb\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 1;
    buf.insert_str("x");
    buf.compute_diff_if_needed();
    assert_eq!(buf.line_diff_status(0), LineDiffStatus::Modified);

    // Simulate save
    buf.mark_clean();
    buf.compute_diff_if_needed();
    assert_eq!(buf.line_diff_status(0), LineDiffStatus::Unchanged);
}

#[test]
fn diff_modified_line_in_context() {
    // Simulate: one line changed in the middle of unchanged code (like lib.rs case)
    let base = "use foo;\nuse bar;\nuse baz;\nuse qux;\n";
    let mut buf = Buffer::from_str("test", base, None);
    // Change "use bar;" → "use bar_v2;"
    buf.cursor.line = 1;
    buf.cursor.col = 7;
    buf.insert_str("_v2");
    buf.compute_diff_if_needed();
    assert_eq!(buf.line_diff_status(0), LineDiffStatus::Unchanged);
    assert_eq!(buf.line_diff_status(1), LineDiffStatus::Modified);
    assert_eq!(buf.line_diff_status(2), LineDiffStatus::Unchanged);
    assert_eq!(buf.line_diff_status(3), LineDiffStatus::Unchanged);
}

#[test]
fn diff_added_block_between_unchanged() {
    // Simulate: new block inserted between existing code
    let base = "fn a() {}\nfn b() {}\n";
    let mut buf = Buffer::from_str("test", base, None);
    buf.cursor.line = 0;
    buf.cursor.col = 9;
    buf.insert_newline();
    buf.insert_str("fn new_func() {}");
    buf.compute_diff_if_needed();
    assert_eq!(buf.line_diff_status(0), LineDiffStatus::Unchanged);
    assert_eq!(buf.line_diff_status(1), LineDiffStatus::Added);
    assert_eq!(buf.line_diff_status(2), LineDiffStatus::Unchanged);
}

#[test]
fn newline_after_brace_with_trailing_spaces() {
    let mut buf = Buffer::from_str("test", "fn main() {   ", None);
    buf.cursor.line = 0;
    buf.cursor.col = 14; // after trailing spaces
    buf.insert_newline();
    // Should detect '{' through trailing whitespace
    assert_eq!(buf.content_as_string(), "fn main() {   \n    ");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);
}
