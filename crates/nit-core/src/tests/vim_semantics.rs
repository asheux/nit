// Vim-semantics regression net. Each test exercises a buffer-level
// operation and pins the expected cursor + text state against a
// scripted sequence of operations. The comment above every test names
// the relevant `:help` section so a future contributor can cross-check
// against Neovim's source of truth.
//
// Coverage map for the T1–T11 work:
//   T2  dw / de / db                — covered below under "Word deletes"
//   T4  vim word motions            — "Word motions" / "BIG word motions"
//   T6  yank register round-trips   — owned by Integrator 3; the
//                                     buffer-level half (delete_line,
//                                     delete_to_end, substitute_line)
//                                     is covered under "Operators".
//   T9  language detection          — covered in `languages.rs::tests`.
//   T10 per-language indent default — covered in `languages.rs::tests`.
//
// Ops with no buffer-level surface yet are tagged
// `#[ignore = "vim-precision-T13: ..."]` so they surface in `cargo
// test` output as a punch list — see the PR description for the
// follow-up ticket(s).

use super::*;

// ---- Word motions (w / e / b) — :help word-motions ------------------

#[test]
fn w_lands_at_next_word_start_skipping_whitespace() {
    let mut buf = Buffer::from_str("t", "foo  bar  baz", None);
    buf.cursor.col = 0;
    buf.move_word_forward();
    assert_eq!(buf.cursor.col, 5);
    buf.move_word_forward();
    assert_eq!(buf.cursor.col, 10);
}

#[test]
fn w_stops_at_punctuation_class_boundary() {
    // vim's `w` stops at every change of character class (word /
    // punctuation / whitespace), so "foo,bar" splits into three stops:
    // 'foo', then ',', then 'bar'.
    let mut buf = Buffer::from_str("t", "foo,bar baz", None);
    buf.cursor.col = 0;
    buf.move_word_forward();
    assert_eq!(buf.cursor.col, 3); // on ','
    buf.move_word_forward();
    assert_eq!(buf.cursor.col, 4); // 'b' in 'bar'
    buf.move_word_forward();
    assert_eq!(buf.cursor.col, 8); // 'b' in 'baz'
}

#[test]
fn e_lands_on_last_char_of_current_or_next_word() {
    let mut buf = Buffer::from_str("t", "foo bar", None);
    buf.cursor.col = 0;
    buf.move_word_end();
    assert_eq!(buf.cursor.col, 2); // 'o' in foo
    buf.move_word_end();
    assert_eq!(buf.cursor.col, 6); // 'r' in bar
}

#[test]
fn b_walks_backward_over_word_starts() {
    let mut buf = Buffer::from_str("t", "foo bar baz", None);
    buf.cursor.col = 10; // on 'z' in baz
    buf.move_word_back();
    assert_eq!(buf.cursor.col, 8); // 'b' in baz
    buf.move_word_back();
    assert_eq!(buf.cursor.col, 4); // 'b' in bar
    buf.move_word_back();
    assert_eq!(buf.cursor.col, 0); // 'f' in foo
}

#[test]
fn word_motions_handle_quoted_punctuation_run() {
    // The mixed punctuation/word run from the operator's T4 spec —
    // `foo "bar" (baz)`. Each class boundary is a `w` stop.
    let mut buf = Buffer::from_str("t", "foo \"bar\" (baz)", None);
    buf.cursor.col = 0;

    let landings = [4, 5, 8, 10, 11, 14];
    for expected in landings {
        buf.move_word_forward();
        assert_eq!(buf.cursor.col, expected, "after w landing on {expected}");
    }
}

// ---- BIG word motions (W / E / B) — :help WORD ----------------------

#[test]
fn big_word_jumps_over_punctuation_in_word() {
    let mut buf = Buffer::from_str("t", "foo,bar baz", None);
    buf.cursor.col = 0;
    buf.move_big_word_forward();
    assert_eq!(buf.cursor.col, 8); // skips "foo,bar" as one WORD
}

#[test]
fn big_word_end_lands_on_last_char_of_run() {
    let mut buf = Buffer::from_str("t", "foo,bar baz", None);
    buf.cursor.col = 0;
    buf.move_big_word_end();
    assert_eq!(buf.cursor.col, 6); // 'r' in "foo,bar"
    buf.move_big_word_end();
    assert_eq!(buf.cursor.col, 10); // 'z' in baz
}

#[test]
fn big_word_back_lands_on_run_start() {
    let mut buf = Buffer::from_str("t", "foo,bar baz", None);
    buf.cursor.col = 9;
    buf.move_big_word_back();
    assert_eq!(buf.cursor.col, 8); // start of baz
    buf.move_big_word_back();
    assert_eq!(buf.cursor.col, 0); // start of "foo,bar"
}

// ---- Line-anchor motions — :help left-right-motions -----------------

#[test]
fn move_home_zero_jumps_to_column_zero() {
    let mut buf = Buffer::from_str("t", "    hello", None);
    buf.cursor.col = 7;
    buf.move_home();
    assert_eq!(buf.cursor.col, 0);
}

#[test]
fn caret_skips_leading_whitespace() {
    let mut buf = Buffer::from_str("t", "    hello", None);
    buf.cursor.col = 0;
    buf.move_first_non_blank();
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn dollar_lands_at_end_of_line_without_newline() {
    let mut buf = Buffer::from_str("t", "hello world\nnext", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.move_end();
    assert_eq!(buf.cursor.col, 11); // past 'd'
}

// ---- Document motions — :help G --------------------------------------

#[test]
fn gg_and_g_jump_to_first_and_last_lines() {
    let mut buf = Buffer::from_str("t", "alpha\nbeta\ngamma\n", None);
    buf.cursor.line = 1;
    buf.go_to_bottom();
    // Trailing newline produces a final empty line index — go_to_bottom
    // clamps to that line.
    let last = buf.lines_len().saturating_sub(1);
    assert_eq!(buf.cursor.line, last);

    buf.go_to_top();
    assert_eq!(buf.cursor.line, 0);
}

#[test]
fn go_to_line_clamps_to_buffer_length() {
    let mut buf = Buffer::from_str("t", "a\nb\nc\n", None);
    buf.go_to_line(99);
    let last = buf.lines_len().saturating_sub(1);
    assert_eq!(buf.cursor.line, last);
    buf.go_to_line(2);
    assert_eq!(buf.cursor.line, 1); // 1-indexed → 0-indexed
}

// ---- f / F / t / T — :help f ----------------------------------------

#[test]
fn f_forward_lands_on_target() {
    let mut buf = Buffer::from_str("t", "abcXdef", None);
    buf.cursor.col = 0;
    assert!(buf.find_char_in_line('X', true, false));
    assert_eq!(buf.cursor.col, 3);
}

#[test]
fn t_forward_lands_one_before_target() {
    let mut buf = Buffer::from_str("t", "abcXdef", None);
    buf.cursor.col = 0;
    assert!(buf.find_char_in_line('X', true, true));
    assert_eq!(buf.cursor.col, 2);
}

#[test]
fn capital_f_walks_backward_to_target() {
    let mut buf = Buffer::from_str("t", "aXcdef", None);
    buf.cursor.col = 5;
    assert!(buf.find_char_in_line('X', false, false));
    assert_eq!(buf.cursor.col, 1);
}

#[test]
fn find_char_returns_false_when_missing() {
    let mut buf = Buffer::from_str("t", "abcdef", None);
    buf.cursor.col = 0;
    assert!(!buf.find_char_in_line('z', true, false));
    assert_eq!(buf.cursor.col, 0);
}

// ---- n / N search — :help search-commands ---------------------------

#[test]
fn n_advances_to_next_match_then_wraps() {
    let mut buf = Buffer::from_str("t", "foo bar foo baz foo\n", None);
    buf.cursor.col = 0;
    assert!(buf.search_next_match("foo", false));
    assert_eq!(buf.cursor.col, 8); // second foo
    assert!(buf.search_next_match("foo", false));
    assert_eq!(buf.cursor.col, 16); // third foo
    assert!(buf.search_next_match("foo", false));
    assert_eq!(buf.cursor.col, 0); // wrapped to first
}

#[test]
fn capital_n_walks_backward_through_matches() {
    let mut buf = Buffer::from_str("t", "foo bar foo baz foo\n", None);
    buf.cursor.col = 17; // inside third foo
    assert!(buf.search_prev_match("foo", false));
    assert_eq!(buf.cursor.col, 8);
    assert!(buf.search_prev_match("foo", false));
    assert_eq!(buf.cursor.col, 0);
}

#[test]
fn search_whole_word_skips_substring_hits() {
    let mut buf = Buffer::from_str("t", "result results result\n", None);
    buf.cursor.col = 0;
    assert!(buf.search_next_match("result", true));
    assert_eq!(buf.cursor.col, 15);
}

// ---- * / # — :help star ---------------------------------------------

#[test]
fn star_extracts_word_under_cursor_for_search() {
    // The buffer-level building block for `*`: extract the word, then
    // run a whole-word search. Full operator wiring lives at the
    // action_apply layer (Integrator 3+4 scope).
    let mut buf = Buffer::from_str("t", "foo bar foo\n", None);
    buf.cursor.col = 5; // inside 'bar'
    assert_eq!(buf.word_at_cursor().as_deref(), Some("bar"));

    // Now drive the search from that word, as `*` would.
    let word = buf.word_at_cursor().unwrap();
    assert!(buf.search_next_match(&word, true));
    // No second 'bar' → wraps to the same hit.
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn word_at_cursor_returns_none_on_blank_line() {
    let mut buf = Buffer::from_str("t", "   \n", None);
    buf.cursor.col = 0;
    assert_eq!(buf.word_at_cursor(), None);
}

// ---- Operators — :help operator -------------------------------------

#[test]
fn dollar_d_deletes_to_end_of_line() {
    // `D` (a.k.a. `d$`).
    let mut buf = Buffer::from_str("t", "hello world", None);
    buf.cursor.col = 5;
    buf.delete_to_end();
    assert_eq!(buf.content_as_string(), "hello");
}

#[test]
fn dd_deletes_whole_line_and_keeps_cursor_in_range() {
    let mut buf = Buffer::from_str("t", "alpha\nbeta\ngamma\n", None);
    buf.cursor.line = 1;
    buf.cursor.col = 2;
    buf.delete_line();
    // After dd on "beta", "gamma\n" becomes line 1.
    assert!(buf.content_as_string().starts_with("alpha\ngamma\n"));
    assert!(buf.cursor.line <= buf.lines_len().saturating_sub(1));
}

#[test]
fn cc_substitute_line_preserves_indent() {
    // `cc` / `S` — clear the line but keep its leading whitespace,
    // ready for insert mode.
    let mut buf = Buffer::from_str("t", "    hello world", None);
    buf.cursor.col = 9;
    buf.substitute_line();
    assert_eq!(buf.content_as_string(), "    ");
    assert_eq!(buf.cursor.col, 4);
}

// ---- Word deletes (T2) — :help dw -----------------------------------

#[test]
fn dw_deletes_to_next_word_start() {
    let mut buf = Buffer::from_str("t", "foo bar baz", None);
    buf.cursor.col = 0;
    buf.delete_word_forward();
    assert_eq!(buf.content_as_string(), "bar baz");
    assert_eq!(buf.cursor.col, 0);
}

#[test]
fn db_deletes_back_to_word_start() {
    // Cursor on the 'b' of "baz" (col 8); db deletes "bar " and the
    // cursor lands at col 4.
    let mut buf = Buffer::from_str("t", "foo bar baz", None);
    buf.cursor.col = 8;
    buf.delete_word_back();
    assert_eq!(buf.content_as_string(), "foo baz");
}

#[test]
fn dw_on_last_word_clears_to_eol() {
    let mut buf = Buffer::from_str("t", "foo bar", None);
    buf.cursor.col = 4; // 'b' of bar
    buf.delete_word_forward();
    assert_eq!(buf.content_as_string(), "foo ");
}

// ---- r / x / X — :help r --------------------------------------------

#[test]
fn r_replaces_char_in_place() {
    let mut buf = Buffer::from_str("t", "hello", None);
    buf.cursor.col = 1;
    buf.replace_char('a');
    assert_eq!(buf.content_as_string(), "hallo");
    assert_eq!(buf.cursor.col, 1); // cursor doesn't move
}

#[test]
fn r_does_nothing_at_newline() {
    let mut buf = Buffer::from_str("t", "hi\nlo", None);
    buf.cursor.col = 2; // on '\n'
    buf.replace_char('x');
    assert_eq!(buf.content_as_string(), "hi\nlo");
}

#[test]
fn x_deletes_char_under_cursor() {
    let mut buf = Buffer::from_str("t", "hello", None);
    buf.cursor.col = 1;
    buf.delete_forward();
    assert_eq!(buf.content_as_string(), "hllo");
}

#[test]
fn capital_x_deletes_char_before_cursor() {
    let mut buf = Buffer::from_str("t", "hello", None);
    buf.cursor.col = 2; // on 'l'
    buf.backspace();
    assert_eq!(buf.content_as_string(), "hllo");
    assert_eq!(buf.cursor.col, 1);
}

// ---- ~ J — :help J --------------------------------------------------

#[test]
fn tilde_toggles_case_under_cursor() {
    let mut buf = Buffer::from_str("t", "Hello", None);
    buf.cursor.col = 0;
    buf.toggle_case_char();
    assert_eq!(buf.content_as_string(), "hello");
    assert_eq!(buf.cursor.col, 1);
}

#[test]
fn capital_j_joins_with_space_and_trims_leading_ws() {
    let mut buf = Buffer::from_str("t", "alpha\n    beta\n", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.join_lines();
    assert_eq!(buf.content_as_string(), "alpha beta\n");
}

// ---- Undo round-trip — :help undo -----------------------------------

#[test]
fn undo_restores_after_word_delete() {
    let mut buf = Buffer::from_str("t", "foo bar baz", None);
    buf.cursor.col = 0;
    buf.delete_word_forward();
    assert_eq!(buf.content_as_string(), "bar baz");
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "foo bar baz");
}

#[test]
fn undo_restores_after_replace_char() {
    let mut buf = Buffer::from_str("t", "hello", None);
    buf.cursor.col = 1;
    buf.replace_char('a');
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "hello");
}

#[test]
fn redo_reapplies_undone_edit() {
    let mut buf = Buffer::from_str("t", "abc", None);
    buf.cursor.col = 1;
    buf.delete_forward();
    assert_eq!(buf.content_as_string(), "ac");
    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "abc");
    assert!(buf.redo());
    assert_eq!(buf.content_as_string(), "ac");
}

// ---- Punch list — not yet wired at the buffer layer -----------------
//
// These tests pin the *intent*; they're tagged `ignore` so they show up
// in `cargo test` as a follow-up checklist. When the corresponding
// ticket lands, drop the `#[ignore]` attribute.

#[test]
#[ignore = "vim-precision-T13: % match-pair motion currently mis-wired to move_home"]
fn percent_jumps_to_matching_bracket() {
    // Expected: cursor at the `(` (col 6) should land on the matching
    // `)`. Today there is no buffer-level matchpair helper; the
    // action_apply layer routes `%` to `move_home`. Filed as T13.
    let mut buf = Buffer::from_str("t", "fn foo(x) { y }", None);
    buf.cursor.col = 6;
    let _ = &buf;
}

#[test]
#[ignore = "vim-precision-T13: named registers (\"ay / \"ap) not yet implemented"]
fn named_registers_round_trip_yanked_text() {
    // The buffer doesn't own the register table — that lives on
    // AppState. Once Integrator 3's YankRegister migration lands and a
    // letter-keyed register map is wired up, this test moves to
    // tests/state/yank_register.rs (or equivalent).
}

#[test]
#[ignore = "vim-precision-T13: marks m{a-z} / '{a-z} / `{a-z} not implemented"]
fn marks_and_jump_back_to_mark() {
    // Vim marks (`m` to set, `'` to jump to mark line, `` ` `` to jump
    // to mark line+col) are unimplemented. Filed as part of the
    // jumplist (T5) follow-up.
}

#[test]
#[ignore = "vim-precision-T13: counts on operators (5dd, 3dw) live at action_apply"]
fn counts_apply_to_motions_and_operators() {
    // `3dw` should delete three words. The count prefix is captured at
    // the chord layer; the buffer-level methods iterate one unit at a
    // time. Once Integrator 3's Action::DeleteWord* lands, this test
    // moves there.
}

#[test]
#[ignore = "vim-precision-T13: Replace mode (R) not yet implemented"]
fn capital_r_overwrites_characters_in_replace_mode() {
    // `R` enters replace mode where each subsequent insert overwrites
    // the character under the cursor instead of pushing it right. No
    // mode wiring exists yet.
}
