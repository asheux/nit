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

// --- Vim motion tests ---

#[test]
fn move_word_forward_skips_whitespace() {
    let mut buf = Buffer::from_str("t", "foo bar baz", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.move_word_forward();
    assert_eq!(buf.cursor.col, 4); // start of 'bar'
    buf.move_word_forward();
    assert_eq!(buf.cursor.col, 8); // start of 'baz'
}

#[test]
fn move_word_forward_stops_at_punctuation() {
    let mut buf = Buffer::from_str("t", "foo, bar", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.move_word_forward();
    assert_eq!(buf.cursor.col, 3); // on ','
    buf.move_word_forward();
    assert_eq!(buf.cursor.col, 5); // start of 'bar'
}

#[test]
fn move_big_word_forward_ignores_punctuation() {
    let mut buf = Buffer::from_str("t", "foo,bar baz", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.move_big_word_forward();
    // Should skip past "foo,bar" entirely and land on "baz"
    assert_eq!(buf.cursor.col, 8);
}

#[test]
fn move_big_word_back_jumps_over_whole_word() {
    let mut buf = Buffer::from_str("t", "foo,bar baz", None);
    buf.cursor.line = 0;
    buf.cursor.col = 10; // on 'a' of baz
    buf.move_big_word_back();
    assert_eq!(buf.cursor.col, 8); // start of baz
    buf.move_big_word_back();
    assert_eq!(buf.cursor.col, 0); // start of "foo,bar"
}

#[test]
fn move_big_word_end_lands_on_last_char() {
    let mut buf = Buffer::from_str("t", "foo bar baz", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.move_big_word_end();
    assert_eq!(buf.cursor.col, 2); // 'o' in foo
    buf.move_big_word_end();
    assert_eq!(buf.cursor.col, 6); // 'r' in bar
}

#[test]
fn move_first_non_blank_skips_leading_whitespace() {
    let mut buf = Buffer::from_str("t", "    hello", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.move_first_non_blank();
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn move_first_non_blank_on_blank_line_stays_at_zero() {
    let mut buf = Buffer::from_str("t", "   \nfoo", None);
    buf.cursor.line = 0;
    buf.cursor.col = 2;
    buf.move_first_non_blank();
    // Line is all whitespace so cursor goes past end to col 3.
    assert_eq!(buf.cursor.col, 3);
}

#[test]
fn move_last_non_blank_ignores_trailing_whitespace() {
    let mut buf = Buffer::from_str("t", "hello    ", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.move_last_non_blank();
    assert_eq!(buf.cursor.col, 4); // 'o' in hello
}

#[test]
fn move_paragraph_down_jumps_to_blank_line() {
    let mut buf = Buffer::from_str("t", "foo\nbar\n\nbaz\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.move_paragraph_down();
    assert_eq!(buf.cursor.line, 2); // blank line
}

#[test]
fn move_paragraph_up_jumps_to_blank_line_above() {
    let mut buf = Buffer::from_str("t", "foo\n\nbar\nbaz\n", None);
    buf.cursor.line = 3;
    buf.cursor.col = 0;
    buf.move_paragraph_up();
    assert_eq!(buf.cursor.line, 1);
}

#[test]
fn move_viewport_top_uses_viewport_offset() {
    let mut buf = Buffer::from_str("t", "a\nb\nc\nd\ne\nf\n", None);
    buf.viewport.height = 3;
    buf.viewport.offset_line = 2;
    buf.cursor.line = 4;
    buf.cursor.col = 0;
    buf.move_viewport_top();
    assert_eq!(buf.cursor.line, 2);
}

#[test]
fn move_viewport_middle_uses_viewport_offset() {
    let mut buf = Buffer::from_str("t", "a\nb\nc\nd\ne\nf\n", None);
    buf.viewport.height = 4;
    buf.viewport.offset_line = 1;
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.move_viewport_middle();
    assert_eq!(buf.cursor.line, 3); // 1 + 4/2 = 3
}

#[test]
fn move_viewport_bottom_uses_viewport_offset_and_height() {
    let mut buf = Buffer::from_str("t", "a\nb\nc\nd\ne\nf\n", None);
    buf.viewport.height = 3;
    buf.viewport.offset_line = 1;
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.move_viewport_bottom();
    assert_eq!(buf.cursor.line, 3); // 1 + 3 - 1 = 3
}

// --- Vim operator tests ---

#[test]
fn delete_to_end_removes_rest_of_line() {
    let mut buf = Buffer::from_str("t", "hello world", None);
    buf.cursor.line = 0;
    buf.cursor.col = 5;
    buf.delete_to_end();
    assert_eq!(buf.content_as_string(), "hello");
}

#[test]
fn delete_to_end_on_empty_line_is_noop() {
    let mut buf = Buffer::from_str("t", "", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.delete_to_end();
    assert_eq!(buf.content_as_string(), "");
}

#[test]
fn substitute_line_clears_content_keeps_indent() {
    let mut buf = Buffer::from_str("t", "    hello world", None);
    buf.cursor.line = 0;
    buf.cursor.col = 9;
    buf.substitute_line();
    assert_eq!(buf.content_as_string(), "    ");
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn join_lines_merges_next_line_with_space() {
    let mut buf = Buffer::from_str("t", "hello\nworld\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.join_lines();
    assert_eq!(buf.content_as_string(), "hello world\n");
    assert_eq!(buf.cursor.line, 0);
    assert_eq!(buf.cursor.col, 5);
}

#[test]
fn join_lines_strips_leading_whitespace_on_next_line() {
    let mut buf = Buffer::from_str("t", "hello\n    world\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.join_lines();
    assert_eq!(buf.content_as_string(), "hello world\n");
}

#[test]
fn join_lines_empty_current_gives_no_leading_space() {
    let mut buf = Buffer::from_str("t", "\nworld\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.join_lines();
    assert_eq!(buf.content_as_string(), "world\n");
}

#[test]
fn toggle_case_char_flips_case_and_advances() {
    let mut buf = Buffer::from_str("t", "Hello", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.toggle_case_char();
    assert_eq!(buf.content_as_string(), "hello");
    assert_eq!(buf.cursor.col, 1);

    buf.toggle_case_char();
    assert_eq!(buf.content_as_string(), "hEllo");
    assert_eq!(buf.cursor.col, 2);
}

#[test]
fn toggle_case_char_non_alpha_still_advances() {
    let mut buf = Buffer::from_str("t", "a1b", None);
    buf.cursor.line = 0;
    buf.cursor.col = 1; // on '1'
    buf.toggle_case_char();
    assert_eq!(buf.content_as_string(), "a1b");
    assert_eq!(buf.cursor.col, 2);
}

#[test]
fn replace_char_swaps_in_place() {
    let mut buf = Buffer::from_str("t", "hello", None);
    buf.cursor.line = 0;
    buf.cursor.col = 1;
    buf.replace_char('a');
    assert_eq!(buf.content_as_string(), "hallo");
    // Cursor stays on the replaced character (vim semantics).
    assert_eq!(buf.cursor.col, 1);
}

#[test]
fn replace_char_does_nothing_on_newline() {
    let mut buf = Buffer::from_str("t", "hi\nlo", None);
    buf.cursor.line = 0;
    buf.cursor.col = 2; // on '\n'
    buf.replace_char('x');
    assert_eq!(buf.content_as_string(), "hi\nlo");
}

// --- Vim find-char tests ---

#[test]
fn find_char_forward_lands_on_target() {
    let mut buf = Buffer::from_str("t", "abcXdef", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    assert!(buf.find_char_in_line('X', true, false));
    assert_eq!(buf.cursor.col, 3);
}

#[test]
fn find_char_forward_returns_false_when_missing() {
    let mut buf = Buffer::from_str("t", "abcdef", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    assert!(!buf.find_char_in_line('z', true, false));
    assert_eq!(buf.cursor.col, 0);
}

#[test]
fn till_char_forward_lands_one_before_target() {
    let mut buf = Buffer::from_str("t", "abcXdef", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    assert!(buf.find_char_in_line('X', true, true));
    assert_eq!(buf.cursor.col, 2);
}

#[test]
fn find_char_back_walks_backwards() {
    let mut buf = Buffer::from_str("t", "aXcdef", None);
    buf.cursor.line = 0;
    buf.cursor.col = 5;
    assert!(buf.find_char_in_line('X', false, false));
    assert_eq!(buf.cursor.col, 1);
}

#[test]
fn till_char_back_lands_one_after_target() {
    let mut buf = Buffer::from_str("t", "aXcdef", None);
    buf.cursor.line = 0;
    buf.cursor.col = 5;
    assert!(buf.find_char_in_line('X', false, true));
    assert_eq!(buf.cursor.col, 2);
}

#[test]
fn find_char_does_not_leave_current_line() {
    let mut buf = Buffer::from_str("t", "abc\nXdef", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    assert!(!buf.find_char_in_line('X', true, false));
    assert_eq!(buf.cursor.col, 0);
}

// --- Vim scroll / viewport tests ---

#[test]
fn scroll_half_page_down_moves_both_cursor_and_offset() {
    let mut buf = Buffer::from_str("t", "a\nb\nc\nd\ne\nf\ng\nh\n", None);
    buf.viewport.height = 4;
    buf.viewport.offset_line = 0;
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.scroll_half_page_down();
    assert_eq!(buf.cursor.line, 2);
    assert_eq!(buf.viewport.offset_line, 2);
}

#[test]
fn scroll_half_page_up_retreats() {
    let mut buf = Buffer::from_str("t", "a\nb\nc\nd\ne\nf\ng\nh\n", None);
    buf.viewport.height = 4;
    buf.viewport.offset_line = 4;
    buf.cursor.line = 5;
    buf.cursor.col = 0;
    buf.scroll_half_page_up();
    assert_eq!(buf.cursor.line, 3);
    assert_eq!(buf.viewport.offset_line, 2);
}

#[test]
fn center_viewport_on_cursor_centers_line() {
    let mut buf = Buffer::from_str("t", "a\nb\nc\nd\ne\nf\ng\nh\n", None);
    buf.viewport.height = 5;
    buf.cursor.line = 5;
    buf.center_viewport_on_cursor();
    // 5 - 5/2 = 5 - 2 = 3
    assert_eq!(buf.viewport.offset_line, 3);
}

#[test]
fn viewport_top_on_cursor_aligns_offset_to_cursor() {
    let mut buf = Buffer::from_str("t", "a\nb\nc\nd\ne\nf\n", None);
    buf.viewport.height = 3;
    buf.cursor.line = 2;
    buf.viewport_top_on_cursor();
    assert_eq!(buf.viewport.offset_line, 2);
}

#[test]
fn viewport_bottom_on_cursor_aligns_cursor_to_bottom() {
    let mut buf = Buffer::from_str("t", "a\nb\nc\nd\ne\nf\n", None);
    buf.viewport.height = 3;
    buf.cursor.line = 4;
    buf.viewport_bottom_on_cursor();
    // 4 - (3 - 1) = 2
    assert_eq!(buf.viewport.offset_line, 2);
}

// --- Undo/redo interaction with new operators ---

#[test]
fn delete_to_end_is_undoable() {
    let mut buf = Buffer::from_str("t", "hello world", None);
    buf.cursor.line = 0;
    buf.cursor.col = 5;
    buf.delete_to_end();
    assert_eq!(buf.content_as_string(), "hello");
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "hello world");
}

#[test]
fn replace_char_is_undoable() {
    let mut buf = Buffer::from_str("t", "hello", None);
    buf.cursor.line = 0;
    buf.cursor.col = 1;
    buf.replace_char('a');
    assert_eq!(buf.content_as_string(), "hallo");
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "hello");
}

#[test]
fn join_lines_is_undoable() {
    let mut buf = Buffer::from_str("t", "hello\nworld\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.join_lines();
    assert_eq!(buf.content_as_string(), "hello world\n");
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "hello\nworld\n");
}
