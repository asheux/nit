use crate::actions::Action;
use crate::buffer::Buffer;
use crate::state::action_apply::apply_action;
use crate::state::AppState;
use crate::test_helpers::temp_dir;

fn state_with(text: &str) -> AppState {
    let root = temp_dir("search_prompt");
    AppState::new(
        root,
        Buffer::from_str("x", text, None),
        Buffer::empty("n", None),
    )
}

fn type_query(state: &mut AppState, query: &str) {
    apply_action(state, Action::SearchPromptOpen);
    for ch in query.chars() {
        apply_action(state, Action::SearchPromptInput(ch));
    }
}

#[test]
fn slash_mixed_case_only_matches_mixed_case() {
    // /Foo (uppercase) is case-sensitive — Foo matches, FOO doesn't.
    let text = "Foo\nFOO\nfoo\n";
    let mut state = state_with(text);
    type_query(&mut state, "Foo");
    apply_action(&mut state, Action::SearchPromptExecute);

    let buf = state.editor_buffer();
    assert_eq!(buf.cursor.line, 0);
    assert_eq!(buf.cursor.col, 0);

    // Smart-case is OFF (uppercase present): the only line-0 match is Foo,
    // line-1 FOO must NOT count as a hit when stepping `n`.
    let matches_line0 = buf.search_line_matches_opt(0, "Foo", false, false);
    let matches_line1 = buf.search_line_matches_opt(1, "Foo", false, false);
    assert_eq!(matches_line0.len(), 1);
    assert_eq!(matches_line1.len(), 0);
}

#[test]
fn slash_lowercase_smart_case_matches_all_cases() {
    let text = "Foo\nFOO\nfoo\n";
    let mut state = state_with(text);
    type_query(&mut state, "foo");
    apply_action(&mut state, Action::SearchPromptExecute);

    let buf = state.editor_buffer();
    // All three lines have one match each under smart-case folding.
    let l0 = buf.search_line_matches_opt(0, "foo", false, true);
    let l1 = buf.search_line_matches_opt(1, "foo", false, true);
    let l2 = buf.search_line_matches_opt(2, "foo", false, true);
    assert_eq!(l0.len(), 1, "Foo should match");
    assert_eq!(l1.len(), 1, "FOO should match");
    assert_eq!(l2.len(), 1, "foo should match");
}

#[test]
fn slash_prompt_open_saves_origin_cursor() {
    let mut state = state_with("hello world\nbar\n");
    state.editor_buffer_mut().cursor.line = 1;
    state.editor_buffer_mut().cursor.col = 2;

    apply_action(&mut state, Action::SearchPromptOpen);
    let prompt = state.search_prompt.as_ref().expect("prompt open");
    let (buf_id, cursor) = prompt.pre_search_cursor.expect("origin saved");
    assert_eq!(buf_id, state.active_editor_buffer_id);
    assert_eq!(cursor.line, 1);
    assert_eq!(cursor.col, 2);
}

#[test]
fn slash_prompt_cancel_restores_cursor() {
    let mut state = state_with("hello world\nhello again\n");
    state.editor_buffer_mut().cursor.line = 0;
    state.editor_buffer_mut().cursor.col = 0;

    apply_action(&mut state, Action::SearchPromptOpen);
    for ch in "world".chars() {
        apply_action(&mut state, Action::SearchPromptInput(ch));
    }
    // Incremental search should have moved the cursor to the first match.
    assert_eq!(state.editor_buffer().cursor.col, 6);

    apply_action(&mut state, Action::SearchPromptCancel);
    // Cancel restores the cursor to where `/` was opened from.
    assert_eq!(state.editor_buffer().cursor.line, 0);
    assert_eq!(state.editor_buffer().cursor.col, 0);
    assert!(state.search_prompt.is_none());
}

#[test]
fn slash_prompt_execute_pushes_to_jumplist() {
    let mut state = state_with("aaa\nbbb\nfoo\n");
    state.editor_buffer_mut().cursor.line = 0;
    state.editor_buffer_mut().cursor.col = 0;
    let before_len = state.jumplist.len();

    type_query(&mut state, "foo");
    apply_action(&mut state, Action::SearchPromptExecute);

    assert_eq!(state.jumplist.len(), before_len + 1);
    let back = state.jumplist.jump_back().expect("jumplist has entry");
    assert_eq!(back.line, 0);
    assert_eq!(back.col, 0);
}

#[test]
fn paste_into_search_prompt_strips_newlines() {
    let mut state = state_with("hello\n");
    apply_action(&mut state, Action::SearchPromptOpen);
    state
        .search_prompt
        .as_mut()
        .expect("prompt")
        .append_paste("multi\nline\ntext");
    let prompt = state.search_prompt.as_ref().unwrap();
    assert_eq!(prompt.input, "multi");
}

#[test]
fn paste_into_command_line_strips_newlines() {
    let mut state = state_with("");
    apply_action(&mut state, Action::CommandPromptOpen);
    state
        .command_line
        .as_mut()
        .expect("cmd open")
        .append_paste("w \nfoo");
    assert_eq!(state.command_line.as_ref().unwrap().input, "w ");
}

#[test]
fn incremental_search_moves_cursor_to_first_match() {
    let mut state = state_with("aaa bbb ccc\nfoo bar\n");
    state.editor_buffer_mut().cursor.line = 0;
    state.editor_buffer_mut().cursor.col = 0;
    type_query(&mut state, "bar");
    // After typing `bar`, the cursor should land on the first match.
    let cur = state.editor_buffer().cursor;
    assert_eq!(cur.line, 1);
    assert_eq!(cur.col, 4);
}

#[test]
fn empty_input_restores_cursor_during_incremental() {
    let mut state = state_with("foo bar\n");
    state.editor_buffer_mut().cursor.line = 0;
    state.editor_buffer_mut().cursor.col = 0;
    type_query(&mut state, "bar");
    assert_eq!(state.editor_buffer().cursor.col, 4);
    // Backspace the query down to empty — cursor returns to origin.
    apply_action(&mut state, Action::SearchPromptBackspace);
    apply_action(&mut state, Action::SearchPromptBackspace);
    apply_action(&mut state, Action::SearchPromptBackspace);
    assert_eq!(state.editor_buffer().cursor.col, 0);
}
