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
