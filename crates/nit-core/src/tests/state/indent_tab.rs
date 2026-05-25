use crate::actions::Action;
use crate::buffer::Buffer;
use crate::mode::Mode;
use crate::state::action_apply::apply_action;
use crate::state::AppState;
use crate::test_helpers::temp_dir;

fn state_with(text: &str) -> AppState {
    let root = temp_dir("indent_tab");
    let mut state = AppState::new(
        root,
        Buffer::from_str("x", text, None),
        Buffer::empty("n", None),
    );
    state.mode = Mode::Insert;
    state
}

#[test]
fn tab_in_space_indented_file_inserts_spaces() {
    // Python-style 4-space indented file: Tab in Insert mode should expand
    // to spaces, not insert a literal `\t`.
    let mut state = state_with("def foo():\n    pass\n");
    let buf = state.editor_buffer_mut();
    buf.cursor.line = 1;
    buf.cursor.col = 0;
    apply_action(&mut state, Action::InsertTab);
    let line = state.editor_buffer().line_as_string(1);
    assert!(
        line.starts_with("    "),
        "expected 4 leading spaces, got {line:?}"
    );
    assert!(!line.contains('\t'), "should not contain tab: {line:?}");
}

#[test]
fn tab_in_tab_indented_file_inserts_tab() {
    // Go-style tab-indented file: Tab inserts a literal `\t`.
    let mut state = state_with("func foo() {\n\treturn\n}\n");
    let buf = state.editor_buffer_mut();
    buf.cursor.line = 2;
    buf.cursor.col = 0;
    apply_action(&mut state, Action::InsertTab);
    let line = state.editor_buffer().line_as_string(2);
    assert!(line.starts_with('\t'), "expected leading tab, got {line:?}");
}

#[test]
fn tab_in_empty_file_falls_back_to_tab() {
    // No indented content to infer from — default falls through to `\t`.
    let mut state = state_with("");
    apply_action(&mut state, Action::InsertTab);
    assert_eq!(state.editor_buffer().content_as_string(), "\t");
}

#[test]
fn tab_in_two_space_indented_file_uses_two_spaces() {
    // 2-space indented JS-style file: Tab inserts exactly 2 spaces (gcd of
    // observed widths).
    let mut state = state_with("function foo() {\n  return 1;\n}\n");
    let buf = state.editor_buffer_mut();
    buf.cursor.line = 2;
    buf.cursor.col = 0;
    apply_action(&mut state, Action::InsertTab);
    let line = state.editor_buffer().line_as_string(2);
    assert!(
        line.starts_with("  "),
        "expected 2 leading spaces, got {line:?}"
    );
    assert!(!line.contains('\t'));
}
