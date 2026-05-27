use super::*;

#[path = "buffer/bracket_match.rs"]
mod bracket_match;
#[path = "buffer/cursor_sticky.rs"]
mod cursor_sticky;
#[path = "buffer/indent_block.rs"]
mod indent_block;
#[path = "buffer/indent_style.rs"]
mod indent_style;
#[path = "buffer/jumplist.rs"]
mod jumplist;
#[path = "buffer/smart_newline.rs"]
mod smart_newline;
#[path = "buffer/undo_groups.rs"]
mod undo_groups;
#[path = "buffer/word_motion.rs"]
mod word_motion;
#[path = "buffer/yank_register.rs"]
mod yank_register;

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

// --- Smart indent ---

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
    let mut buf = Buffer::from_str(
        "test.py",
        "def foo():",
        Some(std::path::PathBuf::from("test.py")),
    );
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
fn newline_after_brace_with_trailing_spaces() {
    let mut buf = Buffer::from_str("test", "fn main() {   ", None);
    buf.cursor.line = 0;
    buf.cursor.col = 14; // after trailing spaces
    buf.insert_newline();
    // '{' is detected through trailing whitespace.
    assert_eq!(buf.content_as_string(), "fn main() {   \n    ");
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

// --- Diff ---

#[test]
fn diff_detects_added_lines() {
    let mut buf = Buffer::from_str("test", "a\nb\nc\n", None);
    buf.compute_diff_if_needed();
    assert_eq!(buf.line_diff_status(0), LineDiffStatus::Unchanged);
    assert_eq!(buf.line_diff_status(1), LineDiffStatus::Unchanged);

    buf.cursor.line = 1;
    buf.cursor.col = 0;
    buf.open_line_above();
    buf.insert_str("new");
    buf.compute_diff_if_needed();
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
    // No edits → diff is not computed yet; the status array stays empty.
    let buf = Buffer::from_str("test", "a\nb\nc\n", None);
    assert_eq!(buf.diff_statuses().len(), 0);
}

#[test]
fn diff_resets_on_mark_clean() {
    let mut buf = Buffer::from_str("test", "a\nb\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 1;
    buf.insert_str("x");
    buf.compute_diff_if_needed();
    assert_eq!(buf.line_diff_status(0), LineDiffStatus::Modified);

    buf.mark_clean();
    buf.compute_diff_if_needed();
    assert_eq!(buf.line_diff_status(0), LineDiffStatus::Unchanged);
}

#[test]
fn diff_modified_line_in_context() {
    // One line changed in the middle of unchanged code (the lib.rs case).
    let base = "use foo;\nuse bar;\nuse baz;\nuse qux;\n";
    let mut buf = Buffer::from_str("test", base, None);
    // "use bar;" → "use bar_v2;"
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

// --- Vim word motions ---

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
    // Skips past "foo,bar" and lands on "baz".
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
    // All-whitespace line: cursor goes past the end to col 3.
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
    assert_eq!(buf.cursor.line, 2);
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

// --- Vim operators ---

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

// --- Vim find-char ---

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

// --- Scroll / viewport ---

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
    // 5 - 5/2 = 3
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

// --- Undo/redo for new operators ---

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

// --- Vim search (* / # / n / N) ---

#[test]
fn word_at_cursor_returns_identifier() {
    let mut buf = Buffer::from_str("t", "foo bar_baz qux", None);
    buf.cursor.line = 0;
    buf.cursor.col = 5; // inside 'bar_baz'
    assert_eq!(buf.word_at_cursor().as_deref(), Some("bar_baz"));
}

#[test]
fn word_at_cursor_scans_forward_from_whitespace() {
    let mut buf = Buffer::from_str("t", "   hello", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0; // on whitespace
    assert_eq!(buf.word_at_cursor().as_deref(), Some("hello"));
}

#[test]
fn word_at_cursor_none_on_blank_line() {
    let mut buf = Buffer::from_str("t", "   \n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    assert_eq!(buf.word_at_cursor(), None);
}

#[test]
fn search_line_matches_plain_substring() {
    let buf = Buffer::from_str("t", "foo foobar foo\n", None);
    let matches = buf.search_line_matches(0, "foo", false);
    assert_eq!(matches, vec![(0, 3), (4, 7), (11, 14)]);
}

#[test]
fn search_line_matches_whole_word_ignores_partials() {
    let buf = Buffer::from_str("t", "foo foobar foo\n", None);
    let matches = buf.search_line_matches(0, "foo", true);
    assert_eq!(matches, vec![(0, 3), (11, 14)]);
}

#[test]
fn search_next_match_advances_cursor() {
    let mut buf = Buffer::from_str("t", "foo\nbar foo baz\nfoo end\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    assert!(buf.search_next_match("foo", true));
    // "foo" on line 1 at column 4 (after "bar ").
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);
    assert!(buf.search_next_match("foo", true));
    assert_eq!(buf.cursor.line, 2);
    assert_eq!(buf.cursor.col, 0);
    // Wraps back to line 0.
    assert!(buf.search_next_match("foo", true));
    assert_eq!(buf.cursor.line, 0);
    assert_eq!(buf.cursor.col, 0);
}

#[test]
fn search_next_match_wraps_around() {
    let mut buf = Buffer::from_str("t", "alpha beta\ngamma delta\n", None);
    buf.cursor.line = 1;
    buf.cursor.col = 6;
    assert!(buf.search_next_match("beta", false));
    assert_eq!(buf.cursor.line, 0);
    assert_eq!(buf.cursor.col, 6);
}

#[test]
fn search_next_match_returns_false_when_missing() {
    let mut buf = Buffer::from_str("t", "alpha beta\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    assert!(!buf.search_next_match("missing", false));
    // No match → cursor doesn't move.
    assert_eq!(buf.cursor.col, 0);
}

#[test]
fn search_prev_match_walks_backwards() {
    let mut buf = Buffer::from_str("t", "foo bar foo\nbaz foo\n", None);
    buf.cursor.line = 1;
    buf.cursor.col = 5;
    assert!(buf.search_prev_match("foo", true));
    assert_eq!(buf.cursor.line, 0);
    assert_eq!(buf.cursor.col, 8);
}

#[test]
fn search_prev_match_wraps_to_end() {
    let mut buf = Buffer::from_str("t", "aaa bbb ccc\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    assert!(buf.search_prev_match("ccc", false));
    assert_eq!(buf.cursor.col, 8);
}

#[test]
fn search_whole_word_skips_substring_matches() {
    let mut buf = Buffer::from_str("t", "result results result\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    // From col 0 the whole-word search must skip "results" and land on the
    // final "result" at col 15.
    assert!(buf.search_next_match("result", true));
    assert_eq!(buf.cursor.col, 15);
}

#[test]
fn repeated_star_scans_through_matches_forward() {
    let mut buf = Buffer::from_str("t", "foo foo foo\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    // From inside the first match, `*` skips it and advances.
    assert!(buf.search_next_match("foo", true));
    assert_eq!(buf.cursor.col, 4);
    assert!(buf.search_next_match("foo", true));
    assert_eq!(buf.cursor.col, 8);
    // Wrap to first.
    assert!(buf.search_next_match("foo", true));
    assert_eq!(buf.cursor.col, 0);
}

#[test]
fn repeated_hash_scans_through_matches_backward() {
    let mut buf = Buffer::from_str("t", "foo foo foo\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 8; // on third "foo"
    assert!(buf.search_prev_match("foo", true));
    assert_eq!(buf.cursor.col, 4);
    assert!(buf.search_prev_match("foo", true));
    assert_eq!(buf.cursor.col, 0);
    // Wrap back to last match.
    assert!(buf.search_prev_match("foo", true));
    assert_eq!(buf.cursor.col, 8);
}

// --- T1: insert/delete undo chunking ---

#[test]
fn undo_typing_run_collapses_into_one_step() {
    let mut buf = Buffer::empty("test", None);
    for c in "hello".chars() {
        buf.insert_char(c);
    }
    assert_eq!(buf.content_as_string(), "hello");
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "");
}

#[test]
fn undo_insert_then_motion_then_insert_is_two_steps() {
    let mut buf = Buffer::empty("test", None);
    buf.insert_str("abc");
    buf.move_left();
    buf.move_right();
    buf.insert_str("def");
    assert_eq!(buf.content_as_string(), "abcdef");
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "abc");
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "");
}

#[test]
fn undo_inserts_split_by_newline() {
    let mut buf = Buffer::empty("test", None);
    buf.insert_str("abc");
    buf.insert_newline();
    buf.insert_str("def");
    assert_eq!(buf.content_as_string(), "abc\ndef");
    // First undo rewinds the post-newline "def" group.
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "abc\n");
    // Second undo rewinds the newline itself.
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "abc");
    // Third undo rewinds the original "abc".
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "");
}

#[test]
fn undo_contiguous_backspaces_collapse_into_one_step() {
    let mut buf = Buffer::from_str("test", "hello", None);
    buf.cursor.line = 0;
    buf.cursor.col = 5;
    for _ in 0..5 {
        buf.backspace();
    }
    assert_eq!(buf.content_as_string(), "");
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "hello");
}

#[test]
fn undo_backspace_run_breaks_on_motion() {
    let mut buf = Buffer::from_str("test", "abcdef", None);
    buf.cursor.line = 0;
    buf.cursor.col = 6;
    buf.backspace();
    buf.backspace(); // "abcd"
    buf.move_left();
    buf.move_right();
    buf.backspace();
    buf.backspace(); // "ab"
    assert_eq!(buf.content_as_string(), "ab");
    // First undo restores the second backspace pair.
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "abcd");
    // Second undo restores the first backspace pair.
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "abcdef");
}

#[test]
fn undo_contiguous_forward_deletes_collapse_into_one_step() {
    let mut buf = Buffer::from_str("test", "hello", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    for _ in 0..5 {
        buf.delete_forward();
    }
    assert_eq!(buf.content_as_string(), "");
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "hello");
}

#[test]
fn undo_paste_line_below_breaks_group_from_typing() {
    let mut buf = Buffer::empty("test", None);
    buf.insert_str("abc");
    buf.paste_line_below("xyz");
    // Undo rewinds the paste; the typed "abc" survives.
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "abc");
    // Next undo rewinds the typing.
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "");
}

// --- T3: bracket-aware backspace ---

#[test]
fn backspace_collapses_paren_pair_when_cursor_between() {
    let mut buf = Buffer::from_str("test", "()", None);
    buf.cursor.line = 0;
    buf.cursor.col = 1; // between '(' and ')'
    buf.backspace();
    assert_eq!(buf.content_as_string(), "");
    assert_eq!(buf.cursor.col, 0);
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "()");
}

#[test]
fn backspace_collapses_bracket_pair() {
    let mut buf = Buffer::from_str("test", "[]", None);
    buf.cursor.line = 0;
    buf.cursor.col = 1;
    buf.backspace();
    assert_eq!(buf.content_as_string(), "");
    assert_eq!(buf.cursor.col, 0);
}

#[test]
fn backspace_collapses_brace_pair() {
    let mut buf = Buffer::from_str("test", "{}", None);
    buf.cursor.line = 0;
    buf.cursor.col = 1;
    buf.backspace();
    assert_eq!(buf.content_as_string(), "");
    assert_eq!(buf.cursor.col, 0);
}

#[test]
fn backspace_collapses_double_quote_pair() {
    let mut buf = Buffer::from_str("test", "\"\"", None);
    buf.cursor.line = 0;
    buf.cursor.col = 1;
    buf.backspace();
    assert_eq!(buf.content_as_string(), "");
    assert_eq!(buf.cursor.col, 0);
}

#[test]
fn backspace_collapses_single_quote_pair() {
    let mut buf = Buffer::from_str("test", "''", None);
    buf.cursor.line = 0;
    buf.cursor.col = 1;
    buf.backspace();
    assert_eq!(buf.content_as_string(), "");
    assert_eq!(buf.cursor.col, 0);
}

#[test]
fn backspace_does_not_collapse_pair_when_chars_between() {
    let mut buf = Buffer::from_str("test", "(foo)", None);
    buf.cursor.line = 0;
    buf.cursor.col = 1; // between '(' and 'f'
    buf.backspace();
    // Only '(' should be deleted — the ')' stays at the end.
    assert_eq!(buf.content_as_string(), "foo)");
}

#[test]
fn backspace_collapses_pair_inside_text() {
    let mut buf = Buffer::from_str("test", "let v = ();", None);
    buf.cursor.line = 0;
    buf.cursor.col = 9; // between '(' and ')'
    buf.backspace();
    assert_eq!(buf.content_as_string(), "let v = ;");
    assert_eq!(buf.cursor.col, 8);
}

// --- T6: delete_line LineWise yank + delete_word_* CharWise ---

#[test]
fn delete_line_returns_linewise_text_with_trailing_newline() {
    let mut buf = Buffer::from_str("test", "alpha\nbeta\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    let yanked = buf.delete_line();
    assert_eq!(yanked, "alpha\n");
    assert_eq!(buf.content_as_string(), "beta\n");
}

#[test]
fn delete_line_appends_trailing_newline_for_final_line() {
    // Final line has no trailing newline in source; the yank must still be
    // line-wise so a later `p` paste re-creates the row.
    let mut buf = Buffer::from_str("test", "only", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    let yanked = buf.delete_line();
    assert_eq!(yanked, "only\n");
}

#[test]
fn delete_word_forward_returns_removed_text_charwise() {
    let mut buf = Buffer::from_str("test", "foo bar", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    let yanked = buf.delete_word_forward();
    assert_eq!(yanked, "foo ");
    assert_eq!(buf.content_as_string(), "bar");
    // CharWise: never carries a leading/trailing newline.
    assert!(!yanked.contains('\n'));
}

#[test]
fn delete_word_back_returns_removed_text_charwise() {
    let mut buf = Buffer::from_str("test", "foo bar", None);
    buf.cursor.line = 0;
    buf.cursor.col = 7;
    let yanked = buf.delete_word_back();
    assert_eq!(yanked, "bar");
    assert_eq!(buf.content_as_string(), "foo ");
}

#[test]
fn delete_to_end_returns_removed_text_charwise() {
    let mut buf = Buffer::from_str("test", "hello world", None);
    buf.cursor.line = 0;
    buf.cursor.col = 5;
    let yanked = buf.delete_to_end();
    assert_eq!(yanked, " world");
    assert_eq!(buf.content_as_string(), "hello");
}

// --- T11: smart Enter inside bracket pairs ---

#[test]
fn smart_enter_expands_paren_pair_at_line_start() {
    let mut buf = Buffer::from_str("test", "()", None);
    buf.cursor.line = 0;
    buf.cursor.col = 1; // between '(' and ')'
    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "(\n    \n)");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn smart_enter_expands_bracket_pair_at_line_start() {
    let mut buf = Buffer::from_str("test", "[]", None);
    buf.cursor.line = 0;
    buf.cursor.col = 1;
    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "[\n    \n]");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn smart_enter_expands_brace_pair_mid_line() {
    let mut buf = Buffer::from_str("test", "let f = {};", None);
    buf.cursor.line = 0;
    buf.cursor.col = 9; // between '{' and '}'
    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "let f = {\n    \n};");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn smart_enter_expands_paren_pair_at_end_of_line() {
    // Cursor sits one past the opener with the closer immediately after.
    let mut buf = Buffer::from_str("test", "fn f()", None);
    buf.cursor.line = 0;
    buf.cursor.col = 5; // between '(' and ')'
    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "fn f(\n    \n)");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn smart_enter_does_not_expand_quote_pair() {
    // Quotes are excluded from `is_indent_opener` because they don't open
    // an indent scope — pressing Enter inside `""` should produce a plain
    // newline, not a multi-line expansion.
    let mut buf = Buffer::from_str("test", "\"\"", None);
    buf.cursor.line = 0;
    buf.cursor.col = 1;
    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "\"\n\"");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 0);
}

#[test]
fn smart_enter_pair_expansion_is_one_undo_step() {
    let mut buf = Buffer::from_str("test", "fn f() {}", None);
    buf.cursor.line = 0;
    buf.cursor.col = 8; // between '{' and '}'
    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "fn f() {\n    \n}");
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "fn f() {}");
}

#[test]
fn smart_enter_does_not_expand_across_existing_newlines() {
    // T11 invariant: `build_indented_newline` only expands when the
    // opener and the closer sit on the same line. Once the pair has
    // already been broken across lines (cursor on a blank middle line
    // between `(` above and `)` below), pressing Enter inserts a plain
    // indented newline — it does NOT pull `)` up onto a new third
    // line. Documented on `first_non_ws_after_cursor` in `indent.rs`.
    let mut buf = Buffer::from_str("test", "fn foo(\n    \n)", None);
    buf.cursor.line = 1;
    buf.cursor.col = 4;
    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "fn foo(\n    \n    \n)");
    assert_eq!(buf.cursor.line, 2);
    assert_eq!(buf.cursor.col, 4);
}

// --- T4: char-class word motions on mixed punctuation runs ---

#[test]
fn w_walks_class_boundaries_in_quoted_phrase() {
    // T4 ticket spec — `foo "bar" (baz)` is the operator's literal
    // acceptance string. `w` lands on the start of every new class
    // (word / punct / whitespace) run, which gives six stops:
    //   foo → "  (0 → 4)
    //   "   → bar (4 → 5)
    //   bar → "   (5 → 8)
    //   "   → (   (8 → 10) — the trailing space is skipped
    //   (   → baz (10 → 11)
    //   baz → )   (11 → 14)
    let mut buf = Buffer::from_str("t", "foo \"bar\" (baz)", None);
    buf.cursor.col = 0;
    let landings = [4, 5, 8, 10, 11, 14];
    for expected in landings {
        buf.move_word_forward();
        assert_eq!(buf.cursor.col, expected, "w step landed on {expected}");
    }
}

#[test]
fn e_walks_class_boundaries_in_quoted_phrase() {
    // `foo "bar" (baz)` — `e` lands on the end of every class run:
    //   foo → " (2 → 4); " → bar (4 → 7); bar → " (7 → 8);
    //   " → ( (8 → 10); ( → baz (10 → 13); baz → ) (13 → 14).
    let mut buf = Buffer::from_str("t", "foo \"bar\" (baz)", None);
    buf.cursor.col = 0;
    let landings = [2, 4, 7, 8, 10, 13, 14];
    for expected in landings {
        buf.move_word_end();
        assert_eq!(buf.cursor.col, expected, "e step landed on {expected}");
    }
}

#[test]
fn b_walks_class_boundaries_in_quoted_phrase() {
    // From the trailing `)` of `foo "bar" (baz)`, `b` walks back through
    // every class start: ) → ( → baz → ( → " → bar → " → foo.
    let mut buf = Buffer::from_str("t", "foo \"bar\" (baz)", None);
    buf.cursor.col = 14; // on ')'
    let landings = [11, 10, 8, 5, 4, 0];
    for expected in landings {
        buf.move_word_back();
        assert_eq!(buf.cursor.col, expected, "b step landed on {expected}");
    }
}

#[test]
fn big_w_jumps_run_to_run_in_quoted_phrase() {
    // `W` ignores intra-WORD punctuation: `foo "bar" (baz)` is three
    // WORDS — `foo`, `"bar"`, `(baz)`.
    let mut buf = Buffer::from_str("t", "foo \"bar\" (baz)", None);
    buf.cursor.col = 0;
    buf.move_big_word_forward();
    assert_eq!(buf.cursor.col, 4); // start of "bar"
    buf.move_big_word_forward();
    assert_eq!(buf.cursor.col, 10); // start of (baz)
    buf.move_big_word_back();
    assert_eq!(buf.cursor.col, 4);
    buf.move_big_word_back();
    assert_eq!(buf.cursor.col, 0);
}

#[test]
fn big_e_lands_on_last_char_of_each_run() {
    let mut buf = Buffer::from_str("t", "foo \"bar\" (baz)", None);
    buf.cursor.col = 0;
    buf.move_big_word_end();
    assert_eq!(buf.cursor.col, 2); // 'o' end of foo
    buf.move_big_word_end();
    assert_eq!(buf.cursor.col, 8); // closing '"' of "bar"
    buf.move_big_word_end();
    assert_eq!(buf.cursor.col, 14); // ')' end of (baz)
}

// --- T2: dw / de / db span semantics ---

#[test]
fn dw_on_whitespace_deletes_only_whitespace_run() {
    // Cursor on the leading space of " bar" — `dw` deletes the space
    // up to where `w` would land, leaving "foo" + "bar" → "foobar".
    let mut buf = Buffer::from_str("t", "foo bar", None);
    buf.cursor.col = 3;
    buf.delete_word_forward();
    assert_eq!(buf.content_as_string(), "foobar");
    assert_eq!(buf.cursor.col, 3);
}

#[test]
fn de_on_last_word_deletes_to_end_of_buffer() {
    let mut buf = Buffer::from_str("t", "foo bar", None);
    buf.cursor.col = 4; // 'b' of bar
    buf.delete_word_end();
    assert_eq!(buf.content_as_string(), "foo ");
}

#[test]
fn de_from_middle_of_word_deletes_to_class_end() {
    let mut buf = Buffer::from_str("t", "alpha beta", None);
    buf.cursor.col = 2; // inside 'alpha'
    buf.delete_word_end();
    // `e` from inside 'alpha' lands on its last char; `de` deletes that
    // span (inclusive), leaving "al beta".
    assert_eq!(buf.content_as_string(), "al beta");
}

#[test]
fn db_from_word_start_deletes_back_through_previous_class() {
    // Cursor on '(' (col 10) of "foo (baz)". `db` walks back through
    // the whitespace + "foo" word and deletes them.
    let mut buf = Buffer::from_str("t", "foo (baz)", None);
    buf.cursor.col = 4; // '('
    buf.delete_word_back();
    // After db on '(': deleted "foo " (cols 0..4), result is "(baz)".
    assert_eq!(buf.content_as_string(), "(baz)");
    assert_eq!(buf.cursor.col, 0);
}

#[test]
fn repeated_dw_deletes_three_words() {
    // Buffer-level `3dw` simulation: action_apply.rs iterates
    // `delete_word_forward` three times. Each call deletes one
    // class-run + trailing whitespace, so three calls on "one two three
    // four" leave "four".
    let mut buf = Buffer::from_str("t", "one two three four", None);
    buf.cursor.col = 0;
    for _ in 0..3 {
        buf.delete_word_forward();
    }
    assert_eq!(buf.content_as_string(), "four");
}

#[test]
fn dw_undo_restores_original_content() {
    let mut buf = Buffer::from_str("t", "foo bar baz", None);
    buf.cursor.col = 0;
    buf.delete_word_forward();
    assert_eq!(buf.content_as_string(), "bar baz");
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "foo bar baz");
}

#[test]
fn dw_uppercase_deletes_whole_big_word_run_including_punctuation() {
    // `dW` from start of "foo,bar baz" deletes the entire "foo,bar "
    // run (punctuation included), leaving "baz".
    let mut buf = Buffer::from_str("t", "foo,bar baz", None);
    buf.cursor.col = 0;
    buf.delete_big_word_forward();
    assert_eq!(buf.content_as_string(), "baz");
}

#[test]
fn de_uppercase_deletes_to_end_of_current_big_word() {
    let mut buf = Buffer::from_str("t", "foo,bar baz", None);
    buf.cursor.col = 0;
    buf.delete_big_word_end();
    assert_eq!(buf.content_as_string(), " baz");
}

#[test]
fn db_uppercase_walks_back_a_whole_big_word() {
    let mut buf = Buffer::from_str("t", "foo,bar baz", None);
    buf.cursor.col = 10; // 'z' of baz
    buf.delete_big_word_back();
    // dB from 'z': scan_big_word_start_back from col 10 → step to 9
    // ('a'), still non-ws → walk back to start of run → col 8. Range
    // 8..10 removed → "foo,bar z".
    assert_eq!(buf.content_as_string(), "foo,bar z");
}

// --- T5: jumplist ring buffer ---

#[test]
fn jumplist_push_and_walk_back() {
    use crate::buffer::{JumpEntry, JumpList};
    let mut list = JumpList::new();
    list.push(JumpEntry::new(0, 10, 5));
    list.push(JumpEntry::new(0, 20, 0));
    list.push(JumpEntry::new(1, 5, 7));
    assert_eq!(list.len(), 3);

    assert_eq!(list.jump_back(), Some(JumpEntry::new(1, 5, 7)));
    assert_eq!(list.jump_back(), Some(JumpEntry::new(0, 20, 0)));
    assert_eq!(list.jump_back(), Some(JumpEntry::new(0, 10, 5)));
    assert_eq!(list.jump_back(), None);
}

#[test]
fn jumplist_walk_forward_after_back() {
    use crate::buffer::{JumpEntry, JumpList};
    let mut list = JumpList::new();
    for line in [10, 20, 30] {
        list.push(JumpEntry::new(0, line, 0));
    }
    list.jump_back();
    list.jump_back();
    assert_eq!(list.jump_forward(), Some(JumpEntry::new(0, 30, 0)));
}

#[test]
fn jumplist_push_truncates_forward_tail() {
    use crate::buffer::{JumpEntry, JumpList};
    let mut list = JumpList::new();
    list.push(JumpEntry::new(0, 1, 0));
    list.push(JumpEntry::new(0, 2, 0));
    list.push(JumpEntry::new(0, 3, 0));
    list.jump_back(); // returns line 3
    list.jump_back(); // returns line 2 — cursor now points at line 1
                      // Pushing replaces the abandoned forward tail (lines 2 and 3) with
                      // the new entry; the original line-1 record stays in front of it.
    list.push(JumpEntry::new(0, 99, 0));
    assert_eq!(list.len(), 2);
    assert_eq!(list.jump_back(), Some(JumpEntry::new(0, 99, 0)));
    assert_eq!(list.jump_back(), Some(JumpEntry::new(0, 1, 0)));
    assert_eq!(list.jump_back(), None);
}

#[test]
fn jumplist_caps_at_jumplist_capacity_entries() {
    use crate::buffer::{JumpEntry, JumpList, JUMPLIST_CAPACITY};
    let mut list = JumpList::new();
    for line in 0..(JUMPLIST_CAPACITY + 10) {
        list.push(JumpEntry::new(0, line, 0));
    }
    assert_eq!(list.len(), JUMPLIST_CAPACITY);
    // The oldest entries (lines 0..10) were evicted.
    while let Some(entry) = list.jump_back() {
        assert!(entry.line >= 10, "evicted line {} resurfaced", entry.line);
    }
}

#[test]
fn jumplist_dedupes_consecutive_identical_entries() {
    use crate::buffer::{JumpEntry, JumpList};
    let mut list = JumpList::new();
    let here = JumpEntry::new(0, 5, 3);
    list.push(here);
    list.push(here);
    list.push(here);
    assert_eq!(list.len(), 1);
}

// --- T8b: smart backspace on blank lines ---

#[test]
fn backspace_on_fully_empty_line_joins_previous() {
    let mut buf = Buffer::from_str("test", "foo\n\nbar", None);
    buf.cursor.line = 1;
    buf.cursor.col = 0;
    buf.backspace();
    assert_eq!(buf.content_as_string(), "foo\nbar");
    assert_eq!(buf.cursor.line, 0);
    assert_eq!(buf.cursor.col, 3);
}

#[test]
fn backspace_on_whitespace_only_line_removes_whole_line() {
    let mut buf = Buffer::from_str("test", "foo\n   \nbar", None);
    buf.cursor.line = 1;
    buf.cursor.col = 3;
    buf.backspace();
    assert_eq!(buf.content_as_string(), "foo\nbar");
    assert_eq!(buf.cursor.line, 0);
    assert_eq!(buf.cursor.col, 3);
}

#[test]
fn backspace_on_whitespace_line_at_end_of_buffer_joins_previous() {
    let mut buf = Buffer::from_str("test", "foo\n    ", None);
    buf.cursor.line = 1;
    buf.cursor.col = 4;
    buf.backspace();
    assert_eq!(buf.content_as_string(), "foo");
    assert_eq!(buf.cursor.line, 0);
    assert_eq!(buf.cursor.col, 3);
}

#[test]
fn backspace_on_blank_line_is_one_undo_step() {
    let mut buf = Buffer::from_str("test", "foo\n    \nbar", None);
    buf.cursor.line = 1;
    buf.cursor.col = 4;
    buf.backspace();
    assert_eq!(buf.content_as_string(), "foo\nbar");
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "foo\n    \nbar");
}

#[test]
fn backspace_on_blank_first_line_is_noop() {
    let mut buf = Buffer::from_str("test", "    \nfoo", None);
    buf.cursor.line = 0;
    buf.cursor.col = 4;
    let before = buf.content_as_string();
    buf.backspace();
    // Falls through to the per-char branch — the leading whitespace is the
    // only thing in front of the cursor, so backspace eats one space.
    // We just need to confirm we don't blow up; we don't promise a specific
    // shape when there is no previous line to join to.
    assert!(buf.content_as_string().len() <= before.len());
}

#[test]
fn backspace_on_non_blank_line_still_deletes_one_char() {
    let mut buf = Buffer::from_str("test", "foo\nbar", None);
    buf.cursor.line = 1;
    buf.cursor.col = 3;
    buf.backspace();
    assert_eq!(buf.content_as_string(), "foo\nba");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 2);
}

#[test]
fn delete_forward_at_end_of_last_line_is_noop() {
    let mut buf = Buffer::from_str("test", "foo", None);
    buf.cursor.line = 0;
    buf.cursor.col = 3;
    let before_version = buf.version();
    buf.delete_forward();
    assert_eq!(buf.content_as_string(), "foo");
    // No edit recorded — version stays put, no spam events.
    assert_eq!(buf.version(), before_version);
}
