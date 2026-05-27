use std::fs;
use std::path::PathBuf;

use crate::actions::Action;
use crate::buffer::{Buffer, JumpEntry, JumpList};
use crate::cursor::Cursor;
use crate::state::action_apply::apply_action;
use crate::state::jumplist::{
    apply_step, push_cursor, step_back, step_forward, JumpDirection, JumpStepOutcome,
};
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

/// Build a workspace with two files on disk plus an `AppState` whose
/// active editor buffer points at `file_a`. Returns the absolute paths
/// so tests can dispatch `Action::OpenFile` against them.
fn state_with_two_files(label: &str, body_a: &str, body_b: &str) -> (AppState, PathBuf, PathBuf) {
    let root = temp_dir(label);
    let file_a = root.join("a.txt");
    let file_b = root.join("b.txt");
    fs::write(&file_a, body_a).unwrap();
    fs::write(&file_b, body_b).unwrap();
    let state = AppState::new(
        root,
        Buffer::from_str("a.txt", body_a, Some(file_a.clone())),
        Buffer::empty("n", None),
    );
    (state, file_a, file_b)
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

// --- T3: cross-buffer jumplist (NITTree / Ctrl-P / Ctrl-F / :e) ---

#[test]
fn open_file_pushes_pre_open_cursor_onto_jumplist() {
    // Opening a new file via the picker / NITTree / :e must record
    // where the cursor was so Ctrl-O can return to it.
    let (mut state, file_a, file_b) =
        state_with_two_files("open-pushes", "alpha line\nsecond line\n", "beta\n");
    state.editor_buffer_mut().cursor.line = 1;
    state.editor_buffer_mut().cursor.col = 4;
    let buffer_a = state.active_editor_buffer_id;
    assert_eq!(state.editor_buffer().path(), Some(&file_a));

    let before = state.jumplist.len();
    apply_action(&mut state, Action::OpenFile(file_b.clone()));
    assert_eq!(state.editor_buffer().path(), Some(&file_b));
    assert_eq!(state.jumplist.len(), before + 1);

    let entry = state.jumplist.last().expect("jumplist entry");
    assert_eq!(entry.buffer_id, buffer_a);
    assert_eq!(entry.line, 1);
    assert_eq!(entry.col, 4);
}

#[test]
fn opening_already_focused_file_does_not_push() {
    // Re-opening the same file is a UI no-op for the jumplist; without
    // this guard a habitual Ctrl-P → Enter on the current file would
    // flood the ring with self-entries.
    let (mut state, file_a, _) =
        state_with_two_files("open-same", "alpha\nbeta\ngamma\n", "ignored\n");
    state.editor_buffer_mut().cursor.line = 2;
    state.editor_buffer_mut().cursor.col = 0;

    let before = state.jumplist.len();
    apply_action(&mut state, Action::OpenFile(file_a));
    assert_eq!(state.jumplist.len(), before);
}

#[test]
fn ctrl_o_after_open_file_returns_to_original_buffer() {
    // End-to-end: open file B from file A, then jump back. Active
    // buffer must flip back to file A with the saved cursor.
    let (mut state, file_a, file_b) =
        state_with_two_files("xbuf-back", "alpha line\nsecond line\n", "beta\n");
    state.editor_buffer_mut().cursor.line = 1;
    state.editor_buffer_mut().cursor.col = 4;
    let buffer_a = state.active_editor_buffer_id;

    apply_action(&mut state, Action::OpenFile(file_b.clone()));
    let buffer_b = state.active_editor_buffer_id;
    assert_ne!(buffer_a, buffer_b);

    apply_action(&mut state, Action::JumpBack);
    assert_eq!(state.active_editor_buffer_id, buffer_a);
    assert_eq!(state.editor_buffer().path(), Some(&file_a));
    assert_eq!(state.editor_buffer().cursor.line, 1);
    assert_eq!(state.editor_buffer().cursor.col, 4);
}

#[test]
fn ctrl_i_after_ctrl_o_returns_to_destination_buffer() {
    // The anchor-on-first-back invariant: after Ctrl-O from a freshest
    // position, Ctrl-I must walk back to the file we just left, not
    // report an empty ring.
    let (mut state, _, file_b) =
        state_with_two_files("xbuf-forward", "alpha\nbeta\n", "first\nsecond\n");
    apply_action(&mut state, Action::OpenFile(file_b.clone()));
    let buffer_b = state.active_editor_buffer_id;
    state.editor_buffer_mut().cursor.line = 1;
    state.editor_buffer_mut().cursor.col = 2;

    apply_action(&mut state, Action::JumpBack);
    assert_ne!(state.active_editor_buffer_id, buffer_b);

    apply_action(&mut state, Action::JumpForward);
    assert_eq!(state.active_editor_buffer_id, buffer_b);
    assert_eq!(state.editor_buffer().path(), Some(&file_b));
    assert_eq!(state.editor_buffer().cursor.line, 1);
    assert_eq!(state.editor_buffer().cursor.col, 2);
}

#[test]
fn jumping_back_anchors_in_buffer_cursor() {
    // Single-buffer round-trip: after a `gg` jump and a free cursor
    // move, Ctrl-O followed by Ctrl-I must restore the post-move
    // position even though it never reached the ring directly.
    let body = (0..20)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut state = state_with(&body);
    state.editor_buffer_mut().cursor.line = 5;
    state.editor_buffer_mut().cursor.col = 3;
    apply_action(&mut state, Action::GoToTop);

    // Free moves don't push to the ring; anchor must rescue this one.
    state.editor_buffer_mut().cursor.line = 10;
    state.editor_buffer_mut().cursor.col = 1;

    apply_action(&mut state, Action::JumpBack);
    assert_eq!(state.editor_buffer().cursor.line, 5);
    assert_eq!(state.editor_buffer().cursor.col, 3);

    apply_action(&mut state, Action::JumpForward);
    assert_eq!(state.editor_buffer().cursor.line, 10);
    assert_eq!(state.editor_buffer().cursor.col, 1);
}

#[test]
fn new_jump_after_back_pop_truncates_forward_history() {
    // Vim / browser-history semantics: a new jump after walking back
    // discards everything past the current position. Without this,
    // Ctrl-I after the new jump would resurrect stale entries.
    let (mut state, _, file_b) =
        state_with_two_files("xbuf-truncate", "alpha\nbeta\n", "ignored\n");
    apply_action(&mut state, Action::OpenFile(file_b.clone()));
    let entries_before = state.jumplist.len();

    apply_action(&mut state, Action::JumpBack);

    // Push a fresh jump while the nav cursor sits mid-ring.
    state.editor_buffer_mut().cursor.line = 0;
    state.editor_buffer_mut().cursor.col = 0;
    apply_action(&mut state, Action::GoToBottom);

    apply_action(&mut state, Action::JumpForward);
    let status = state.status.as_deref().unwrap_or_default();
    assert!(
        status.contains("No newer"),
        "expected empty-forward status, got {status:?}"
    );
    assert!(state.jumplist.len() <= entries_before + 2);
}

#[test]
fn apply_step_skips_stale_cross_buffer_entry() {
    // Direct test against the apply helper: if the entry points at a
    // buffer slot that no longer exists, the walk must skip it instead
    // of bailing out with a stale `Empty` (vim closes :bd buffers but
    // keeps the ring intact, and we mirror that).
    let mut state = state_with("alpha\nbeta\n");
    state.editor_buffer_mut().cursor.line = 1;
    state.editor_buffer_mut().cursor.col = 0;

    let phantom_id = state.buffers.len() + 5;
    state.jumplist.push(JumpEntry::new(phantom_id, 9, 9));

    let outcome = apply_step(&mut state, JumpDirection::Back);
    assert_eq!(outcome, JumpStepOutcome::Empty);
    let status = state.status.as_deref().unwrap_or_default();
    assert!(status.contains("No older"));
}

#[test]
fn apply_step_on_empty_ring_sets_status() {
    let mut state = state_with("alpha\nbeta\n");
    let outcome = apply_step(&mut state, JumpDirection::Back);
    assert_eq!(outcome, JumpStepOutcome::Empty);
    assert_eq!(
        state.status.as_deref(),
        Some("No older jump position"),
        "Ctrl-O on empty ring must surface a helpful message"
    );
}

#[test]
fn jumplist_last_returns_most_recent_entry() {
    let mut list = JumpList::new();
    assert_eq!(list.last(), None);
    push_cursor(&mut list, 0, Cursor::new(1, 0));
    push_cursor(&mut list, 1, Cursor::new(5, 2));
    assert_eq!(list.last(), Some(JumpEntry::new(1, 5, 2)));
}
