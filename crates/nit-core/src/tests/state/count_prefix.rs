//! Vim numeric prefix: `5j` moves 5 lines, `56G` jumps to line 56, etc.
//!
//! Exercises the count-buffer state machine in `apply_action` and the
//! per-motion arms that consume the count. Each test asserts cursor
//! position AND `state.pending_count` afterwards, since cleanup is part
//! of the contract.

use super::*;
use crate::buffer::Buffer;

/// Build a state seeded with N short lines so motion math is easy to
/// reason about: line index = line content.
fn state_with_lines(n: usize) -> AppState {
    let content = (0..n)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut state = AppState::new(
        crate::test_helpers::temp_dir("count-prefix"),
        Buffer::from_str("x", &content, None),
        Buffer::empty("n", None),
    );
    state.mode = Mode::Normal;
    state.focus = PaneId::Editor;
    state
}

#[test]
fn append_count_digit_builds_pending_count() {
    let mut state = state_with_lines(100);
    apply_action(&mut state, Action::AppendCountDigit(5));
    assert_eq!(state.pending_count, Some(5));
    apply_action(&mut state, Action::AppendCountDigit(6));
    assert_eq!(state.pending_count, Some(56));
}

#[test]
fn motion_with_count_repeats_n_times() {
    let mut state = state_with_lines(50);
    assert_eq!(state.editor_buffer().cursor.line, 0);

    apply_action(&mut state, Action::AppendCountDigit(5));
    apply_action(&mut state, Action::MoveDown);

    assert_eq!(state.editor_buffer().cursor.line, 5);
    assert_eq!(
        state.pending_count, None,
        "count must clear after the motion consumes it"
    );
}

#[test]
fn motion_without_count_runs_once() {
    let mut state = state_with_lines(10);
    apply_action(&mut state, Action::MoveDown);
    assert_eq!(state.editor_buffer().cursor.line, 1);
    assert_eq!(state.pending_count, None);
}

#[test]
fn count_clears_when_non_motion_action_fires() {
    let mut state = state_with_lines(10);
    apply_action(&mut state, Action::AppendCountDigit(5));
    assert_eq!(state.pending_count, Some(5));
    // SwitchMode is not a motion → must drop the count rather than
    // leaking it into the next motion the user types.
    apply_action(&mut state, Action::SwitchMode(Mode::Insert));
    assert_eq!(state.pending_count, None);
}

#[test]
fn go_to_bottom_with_count_jumps_to_that_line() {
    // `56G` in vim = jump to line 56 (1-indexed).
    let mut state = state_with_lines(100);
    apply_action(&mut state, Action::AppendCountDigit(5));
    apply_action(&mut state, Action::AppendCountDigit(6));
    apply_action(&mut state, Action::GoToBottom);

    // Line 56 1-indexed = line 55 0-indexed.
    assert_eq!(state.editor_buffer().cursor.line, 55);
    assert_eq!(state.pending_count, None);
}

#[test]
fn go_to_bottom_without_count_jumps_to_last_line() {
    let mut state = state_with_lines(10);
    apply_action(&mut state, Action::GoToBottom);
    // 10 lines = indices 0..=9.
    assert_eq!(state.editor_buffer().cursor.line, 9);
}

#[test]
fn go_to_top_with_count_jumps_to_that_line() {
    // vim: `5gg` jumps to line 5 (not "5 lines back from top").
    let mut state = state_with_lines(10);
    apply_action(&mut state, Action::GoToBottom);
    assert_eq!(state.editor_buffer().cursor.line, 9);

    apply_action(&mut state, Action::AppendCountDigit(5));
    apply_action(&mut state, Action::GoToTop);
    assert_eq!(state.editor_buffer().cursor.line, 4); // 1-indexed line 5
}

#[test]
fn count_caps_at_99999() {
    let mut state = state_with_lines(2);
    // Try to overflow: push way more digits than the cap.
    for _ in 0..10 {
        apply_action(&mut state, Action::AppendCountDigit(9));
    }
    assert_eq!(state.pending_count, Some(99_999));
}

#[test]
fn go_to_line_beyond_eof_clamps_to_last_line() {
    let mut state = state_with_lines(10);
    apply_action(&mut state, Action::AppendCountDigit(9));
    apply_action(&mut state, Action::AppendCountDigit(9));
    apply_action(&mut state, Action::AppendCountDigit(9));
    apply_action(&mut state, Action::GoToBottom);
    // 999 requested, but only 10 lines exist → last line.
    assert_eq!(state.editor_buffer().cursor.line, 9);
}

#[test]
fn command_line_bare_number_jumps_to_that_line() {
    // `:56` should move the cursor to line 56 (1-indexed).
    let mut state = state_with_lines(100);
    assert!(!handle_command_line(&mut state, "56"));
    assert_eq!(state.editor_buffer().cursor.line, 55);
    assert_eq!(state.status.as_deref(), Some("Line 56"));
}

#[test]
fn command_line_bare_number_clamps_to_last_line_when_too_large() {
    let mut state = state_with_lines(10);
    assert!(!handle_command_line(&mut state, "9999"));
    assert_eq!(state.editor_buffer().cursor.line, 9);
}

// --- Count + operator (T2 follow-up: `3dw`, `2db`, `2dW`) ---
//
// These pin the count-prefix → DeleteWord* end-to-end path that the
// `tests/vim_semantics.rs::counts_apply_to_motions_and_operators`
// placeholder was waiting on. Each test seeds a count, fires the
// action, and asserts both the buffer content and the cursor landing.

fn state_with_text(text: &str) -> AppState {
    let mut state = AppState::new(
        crate::test_helpers::temp_dir("count-operator"),
        Buffer::from_str("x", text, None),
        Buffer::empty("n", None),
    );
    state.mode = Mode::Normal;
    state.focus = PaneId::Editor;
    state
}

#[test]
fn three_dw_deletes_three_words_and_leaves_cursor_at_fourth_word() {
    // vim `3dw`: delete three "word + trailing whitespace" runs starting
    // at the cursor; cursor lands on the start of the fourth word.
    let mut state = state_with_text("alpha beta gamma delta epsilon");
    state.editor_buffer_mut().cursor.col = 0;
    apply_action(&mut state, Action::AppendCountDigit(3));
    apply_action(&mut state, Action::DeleteWordForward);
    assert_eq!(state.editor_buffer().content_as_string(), "delta epsilon");
    assert_eq!(state.editor_buffer().cursor.line, 0);
    assert_eq!(state.editor_buffer().cursor.col, 0);
}

#[test]
fn two_db_walks_back_two_words() {
    // vim `2db`: starting from the end of "alpha beta gamma", walk back
    // two word starts. The buffer shrinks to "alpha " and the cursor
    // lands on the leading space of "beta" (col 6, the first char of
    // the removed span).
    let mut state = state_with_text("alpha beta gamma");
    state.editor_buffer_mut().cursor.col = 15; // 'a' in gamma's tail (end)
    apply_action(&mut state, Action::AppendCountDigit(2));
    apply_action(&mut state, Action::DeleteWordBack);
    assert_eq!(state.editor_buffer().content_as_string(), "alpha a");
    assert_eq!(state.editor_buffer().cursor.col, 6);
}

#[test]
fn two_dbig_w_deletes_two_big_words() {
    // vim `2dW`: delete two whitespace-separated runs, even when they
    // contain punctuation. Starts at col 0; first dW removes
    // "foo,bar ", second removes "baz=qux " — the dotted tail "end"
    // survives.
    let mut state = state_with_text("foo,bar baz=qux end");
    state.editor_buffer_mut().cursor.col = 0;
    apply_action(&mut state, Action::AppendCountDigit(2));
    apply_action(&mut state, Action::DeleteBigWordForward);
    assert_eq!(state.editor_buffer().content_as_string(), "end");
    assert_eq!(state.editor_buffer().cursor.col, 0);
}
