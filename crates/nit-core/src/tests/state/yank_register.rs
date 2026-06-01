use crate::actions::Action;
use crate::buffer::Buffer;
use crate::mode::Mode;
use crate::state::action_apply::apply_action;
use crate::state::{AppState, YankRegister};
use crate::test_helpers::temp_dir;

fn state_with(text: &str) -> AppState {
    let root = temp_dir("yank_register");
    AppState::new(
        root,
        Buffer::from_str("x", text, None),
        Buffer::empty("n", None),
    )
}

fn text(state: &AppState) -> String {
    state.editor_buffer().content_as_string()
}

#[test]
fn yy_then_p_pastes_on_new_line_below() {
    // vim `yy` then `p`: line-wise paste lands the cursor at column 0 of
    // the pasted line (one row below the source). See `:help linewise-register`.
    let mut state = state_with("alpha\nbeta\ngamma\n");
    state.editor_buffer_mut().cursor.line = 0;
    state.editor_buffer_mut().cursor.col = 0;

    apply_action(&mut state, Action::YankLine);
    assert!(matches!(
        state.yank_register(),
        Some(YankRegister::LineWise(_))
    ));

    apply_action(&mut state, Action::Paste);
    assert_eq!(text(&state), "alpha\nalpha\nbeta\ngamma\n");
    assert_eq!(state.editor_buffer().cursor.line, 1);
    assert_eq!(state.editor_buffer().cursor.col, 0);
}

#[test]
fn dd_then_p_pastes_on_new_line_below() {
    // vim `dd` then `p`: cursor lands at col 0 of the pasted row, one
    // line below the original cursor.
    let mut state = state_with("first\nsecond\nthird\n");
    state.editor_buffer_mut().cursor.line = 0;

    apply_action(&mut state, Action::DeleteLine);
    assert!(matches!(
        state.yank_register(),
        Some(YankRegister::LineWise(_))
    ));
    assert_eq!(text(&state), "second\nthird\n");
    // After dd the cursor stays on the now-promoted line (line 0).
    assert_eq!(state.editor_buffer().cursor.line, 0);
    assert_eq!(state.editor_buffer().cursor.col, 0);

    apply_action(&mut state, Action::Paste);
    assert_eq!(text(&state), "second\nfirst\nthird\n");
    assert_eq!(state.editor_buffer().cursor.line, 1);
    assert_eq!(state.editor_buffer().cursor.col, 0);
}

#[test]
fn dd_then_p_capital_pastes_line_above() {
    // vim `dd` then `P` puts the cut line back on the current row,
    // pushing existing content down. Cursor lands at col 0 of the
    // restored line.
    let mut state = state_with("alpha\nbeta\n");
    state.editor_buffer_mut().cursor.line = 0;
    apply_action(&mut state, Action::DeleteLine);
    apply_action(&mut state, Action::PasteLineAbove);
    assert_eq!(text(&state), "alpha\nbeta\n");
    assert_eq!(state.editor_buffer().cursor.line, 0);
    assert_eq!(state.editor_buffer().cursor.col, 0);
}

#[test]
fn delete_to_end_then_p_pastes_inline_char_wise() {
    // vim `D` from middle of a word, then `p` should paste the captured
    // tail on the same line, after the cursor. Char-wise: no newline added.
    // Cursor after D lands on the last surviving char ("o" of hello).
    // After p, vim places the cursor on the last char of the pasted run.
    let mut state = state_with("hello world\n");
    state.editor_buffer_mut().cursor.line = 0;
    state.editor_buffer_mut().cursor.col = 5;

    apply_action(&mut state, Action::DeleteToEnd);
    assert!(matches!(
        state.yank_register(),
        Some(YankRegister::CharWise(_))
    ));
    assert_eq!(text(&state), "hello\n");
    assert_eq!(state.editor_buffer().cursor.line, 0);

    apply_action(&mut state, Action::Paste);
    assert_eq!(text(&state), "hello world\n");
    assert_eq!(state.editor_buffer().cursor.line, 0);
}

#[test]
fn yank_selection_visual_paste_inline() {
    let mut state = state_with("foobar\n");
    state.mode = Mode::Visual;
    let buf = state.editor_buffer_mut();
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.set_selection_anchor();
    buf.cursor.col = 3;
    apply_action(&mut state, Action::YankSelection);
    let register = state.yank_register();
    assert!(matches!(register, Some(YankRegister::CharWise(_))));
}

#[test]
fn yank_then_clear_clears_register() {
    let mut state = state_with("alpha\n");
    apply_action(&mut state, Action::YankLine);
    assert!(state.yank_register().is_some());
    state.clear_yank_register();
    assert!(state.yank_register().is_none());
}

fn visual_yank_two_lines(state: &mut AppState) {
    // Select (0,0)..=(1,3): a multi-line slice whose last line ("beta") has
    // no trailing newline — the shape that used to fuse on paste.
    state.mode = Mode::Visual;
    let buf = state.editor_buffer_mut();
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.set_selection_anchor();
    buf.cursor.line = 1;
    buf.cursor.col = 3;
    apply_action(state, Action::YankSelection);
}

#[test]
fn visual_yank_multiline_then_p_does_not_fuse_next_line() {
    // Repro for the `}    args = …` collision: a multi-line visual `y` is
    // classified line-wise, so `p` must open fresh rows rather than running
    // the block's tail into the following line.
    let mut state = state_with("alpha\nbeta\ngamma\n");
    visual_yank_two_lines(&mut state);
    assert!(matches!(
        state.yank_register(),
        Some(YankRegister::LineWise(_))
    ));

    state.editor_buffer_mut().cursor.line = 0;
    apply_action(&mut state, Action::Paste);
    assert_eq!(text(&state), "alpha\nalpha\nbeta\nbeta\ngamma\n");
}

#[test]
fn visual_yank_paste_then_undo_restores_exactly() {
    // The line-wise paste is one undo group: a single undo removes the whole
    // pasted block and restores the buffer byte-for-byte.
    let mut state = state_with("alpha\nbeta\ngamma\n");
    visual_yank_two_lines(&mut state);

    state.editor_buffer_mut().cursor.line = 0;
    apply_action(&mut state, Action::Paste);
    assert_eq!(text(&state), "alpha\nalpha\nbeta\nbeta\ngamma\n");

    apply_action(&mut state, Action::Undo);
    assert_eq!(text(&state), "alpha\nbeta\ngamma\n");
}

#[test]
fn charwise_paste_seals_as_one_undo_group() {
    // The char-wise paste seals append + insert, so a later keystroke is a
    // *separate* undo step — one undo peels off the typing and leaves the
    // pasted run intact. Without the seal the two would coalesce.
    let mut state = state_with("ab\n");
    state.editor_buffer_mut().cursor.line = 0;
    state.editor_buffer_mut().cursor.col = 0;
    state.set_yank_register(YankRegister::CharWise("X".into()));

    apply_action(&mut state, Action::Paste);
    apply_action(&mut state, Action::InsertChar('Y'));
    assert_eq!(text(&state), "aXYb\n");

    apply_action(&mut state, Action::Undo);
    assert_eq!(text(&state), "aXb\n");
}
