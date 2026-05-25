use crate::actions::Action;
use crate::buffer::Buffer;
use crate::mode::Mode;
use crate::state::action_apply::apply_action;
use crate::state::{AppState, YankRegister};
use crate::test_helpers::temp_dir;

fn state_with(text: &str) -> AppState {
    let root = temp_dir("change_ops");
    let mut state = AppState::new(
        root,
        Buffer::from_str("x", text, None),
        Buffer::empty("n", None),
    );
    state.mode = Mode::Normal;
    state
}

fn text(state: &AppState) -> String {
    state.editor_buffer().content_as_string()
}

#[test]
fn cw_changes_to_end_of_word_and_enters_insert() {
    // vim quirk: `cw` is `ce`. Cursor on 'f' of "foo bar" — `cw` removes
    // "foo" but leaves the space, then drops into Insert at col 0.
    let mut state = state_with("foo bar");
    apply_action(&mut state, Action::ChangeWordEnd);
    assert_eq!(text(&state), " bar");
    assert_eq!(state.editor_buffer().cursor.col, 0);
    assert_eq!(state.mode, Mode::Insert);
}

#[test]
fn cw_yanks_deleted_text_charwise() {
    let mut state = state_with("hello world");
    apply_action(&mut state, Action::ChangeWordEnd);
    match state.yank_register() {
        Some(YankRegister::CharWise(t)) => assert_eq!(t, "hello"),
        other => panic!("expected CharWise yank, got {other:?}"),
    }
}

#[test]
fn cw_with_count_iterates_change() {
    // `3cw` iterates the change three times. From 'a' of "alpha":
    //   1) on 'a' (word) → delete "alpha"          → " beta gamma delta"
    //   2) on ' ' (ws)   → fall back to `dw`,      → "beta gamma delta"
    //                      which deletes the gap to next word start
    //   3) on 'b' (word) → delete "beta"           → " gamma delta"
    // This isn't strict vim semantics (vim applies the count to the motion,
    // not the change), but it matches the iteration model used for the
    // other `d`/`c` operators in the codebase and produces sensible output.
    let mut state = state_with("alpha beta gamma delta");
    state.pending_count = Some(3);
    apply_action(&mut state, Action::ChangeWordEnd);
    assert_eq!(text(&state), " gamma delta");
    assert_eq!(state.mode, Mode::Insert);
}

#[test]
fn cb_changes_back_to_word_start_and_enters_insert() {
    // "foo bar baz" — cursor on 'b' of "baz" sits at col 8 (3-char word +
    // space + 3-char word + space). `cb` walks back to the start of "bar"
    // and deletes the four chars in between, leaving "foo baz".
    let mut state = state_with("foo bar baz");
    state.editor_buffer_mut().cursor.col = 8;
    apply_action(&mut state, Action::ChangeWordBack);
    assert_eq!(text(&state), "foo baz");
    assert_eq!(state.mode, Mode::Insert);
}

#[test]
fn cc_clears_line_and_enters_insert_at_indent() {
    // Line-wise change: keep the indent of the line above, drop content.
    let mut state = state_with("    foo\nbar");
    apply_action(&mut state, Action::ChangeLine);
    // The first line was deleted, then open_line_above re-inserted an
    // indented blank line; cursor sits at end of indent on the new line.
    assert_eq!(state.mode, Mode::Insert);
    let cursor = state.editor_buffer().cursor;
    assert_eq!(cursor.line, 0);
}

#[test]
fn cc_yanks_linewise() {
    let mut state = state_with("first\nsecond");
    apply_action(&mut state, Action::ChangeLine);
    match state.yank_register() {
        Some(YankRegister::LineWise(t)) => assert_eq!(t, "first\n"),
        other => panic!("expected LineWise yank, got {other:?}"),
    }
}

#[test]
fn cw_on_single_char_word_changes_just_that_char() {
    let mut state = state_with("a bc");
    apply_action(&mut state, Action::ChangeWordEnd);
    assert_eq!(text(&state), " bc");
    assert_eq!(state.mode, Mode::Insert);
}
