use crate::actions::Action;
use crate::buffer::{Buffer, JumpEntry, JumpList};
use crate::cursor::Cursor;
use crate::state::action_apply::apply_action;
use crate::state::jumplist::{push_cursor, step_back, step_forward, JumpStepOutcome};
use crate::state::AppState;
use crate::test_helpers::temp_dir;

fn state_with(text: &str) -> AppState {
    let root = temp_dir("jumplist");
    AppState::new(
        root,
        Buffer::from_str("x", text, None),
        Buffer::empty("n", None),
    )
}

#[test]
fn gg_pushes_to_jumplist_then_jump_back_returns_origin() {
    let body = (0..20)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut state = state_with(&body);
    let buf = state.editor_buffer_mut();
    buf.cursor.line = 5;
    buf.cursor.col = 3;

    apply_action(&mut state, Action::GoToTop);
    // gg pushed the origin onto the jumplist and the cursor jumped to
    // line 0 (col preserved by `Buffer::go_to_top`).
    assert_eq!(state.editor_buffer().cursor.line, 0);
    assert_eq!(state.jumplist.len(), 1);

    let back = state.jumplist.jump_back().expect("entry available");
    assert_eq!(back.line, 5);
    assert_eq!(back.col, 3);
}

#[test]
fn search_next_pushes_to_jumplist() {
    let mut state = state_with("apple\nbanana\napple pie\n");
    state.editor_buffer_mut().cursor.line = 0;
    state.editor_buffer_mut().cursor.col = 0;
    state.editor_search.term = Some("apple".into());
    state.editor_search.forward = true;

    let before = state.jumplist.len();
    apply_action(&mut state, Action::SearchNext);
    assert_eq!(state.jumplist.len(), before + 1);
}

#[test]
fn star_pushes_to_jumplist() {
    let mut state = state_with("hello world\nhello again\n");
    state.editor_buffer_mut().cursor.line = 0;
    state.editor_buffer_mut().cursor.col = 0;

    let before = state.jumplist.len();
    apply_action(&mut state, Action::SearchWordForward);
    assert_eq!(state.jumplist.len(), before + 1);
}

// --- state::jumplist classifier ---

#[test]
fn in_buffer_step_returns_line_and_col() {
    let mut list = JumpList::new();
    push_cursor(&mut list, 0, Cursor::new(7, 4));
    assert_eq!(
        step_back(&mut list, 0),
        JumpStepOutcome::InBuffer { line: 7, col: 4 }
    );
}

#[test]
fn cross_buffer_step_flagged() {
    let mut list = JumpList::new();
    push_cursor(&mut list, 1, Cursor::new(2, 1));
    assert_eq!(
        step_back(&mut list, 0),
        JumpStepOutcome::CrossBuffer {
            target_buffer_id: 1,
            line: 2,
            col: 1,
        }
    );
}

#[test]
fn empty_ring_returns_empty() {
    let mut list = JumpList::new();
    assert_eq!(step_back(&mut list, 0), JumpStepOutcome::Empty);
    assert_eq!(step_forward(&mut list, 0), JumpStepOutcome::Empty);
}

#[test]
fn forward_step_after_back_works() {
    let mut list = JumpList::new();
    push_cursor(&mut list, 0, Cursor::new(1, 0));
    push_cursor(&mut list, 0, Cursor::new(2, 0));
    push_cursor(&mut list, 0, Cursor::new(3, 0));
    let _ = step_back(&mut list, 0);
    let _ = step_back(&mut list, 0);
    let forward = step_forward(&mut list, 0);
    assert!(matches!(forward, JumpStepOutcome::InBuffer { line: 3, .. }));
}

#[test]
fn jump_step_clamps_col_to_line_visible_chars() {
    // T5: the col stored in JumpEntry is the cursor at push time. A
    // subsequent edit (delete to EOL) can leave the target line shorter
    // than the stored col — Action::JumpBack must clamp so the cursor
    // doesn't land past the visible end of the row.
    let mut state = state_with("hello world\nfoo\n");
    state.editor_buffer_mut().cursor.line = 0;
    state.editor_buffer_mut().cursor.col = 0;
    apply_action(&mut state, Action::GoToBottom);
    let _ = JumpEntry::new(0, 0, 0); // anchor the import for the doc above

    state
        .jumplist
        .push(JumpEntry::new(state.active_editor_buffer_id, 1, 99));
    apply_action(&mut state, Action::JumpBack);
    assert_eq!(state.editor_buffer().cursor.line, 1);
    assert_eq!(state.editor_buffer().cursor.col, 3);
}
