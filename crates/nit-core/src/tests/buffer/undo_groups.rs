//! Granular undo / redo behaviour, pinned per T7.
//!
//! These tests guard the contract that a single `u` rewinds one transaction
//! — not the entire buffer — and that redo replays the most recently undone
//! transaction with the standard branching rule (a fresh edit after undo
//! truncates the redo stack). Boundaries are sealed on cursor motion, mode
//! switches, newlines, pastes, and explicit `begin_undo_group` /
//! `end_undo_group` calls used by block operations.

use crate::buffer::Buffer;

fn buf(text: &str) -> Buffer {
    Buffer::from_str("t", text, None)
}

#[test]
fn single_char_undo_rewinds_one_keystroke_when_isolated() {
    let mut b = buf("");
    b.insert_char('a');
    b.exit_insert_mode();
    assert!(b.undo());
    assert_eq!(b.content_as_string(), "");
    assert!(!b.undo());
}

#[test]
fn word_run_collapses_into_one_undo_step() {
    let mut b = buf("");
    for ch in "hello".chars() {
        b.insert_char(ch);
    }
    b.exit_insert_mode();
    assert_eq!(b.content_as_string(), "hello");
    assert!(b.undo());
    assert_eq!(b.content_as_string(), "");
}

#[test]
fn whitespace_between_words_splits_groups() {
    let mut b = buf("");
    for ch in "hi there".chars() {
        b.insert_char(ch);
    }
    b.exit_insert_mode();
    assert!(b.undo());
    let after_first = b.content_as_string();
    assert!(after_first.starts_with("hi"));
    assert!(!after_first.contains("there"));
    assert!(b.undo());
}

#[test]
fn motion_between_inserts_splits_undo_groups() {
    let mut b = buf("hello world");
    b.cursor.col = 0;
    b.insert_char('X');
    b.move_word_forward();
    b.insert_char('Y');
    b.exit_insert_mode();
    assert!(b.undo());
    let after_first = b.content_as_string();
    assert!(!after_first.contains('Y'));
    assert!(b.undo());
    assert!(!b.content_as_string().contains('X'));
}

#[test]
fn redo_replays_a_single_undone_chunk() {
    let mut b = buf("");
    for ch in "abc".chars() {
        b.insert_char(ch);
    }
    b.exit_insert_mode();
    assert!(b.undo());
    assert_eq!(b.content_as_string(), "");
    assert!(b.redo());
    assert_eq!(b.content_as_string(), "abc");
    assert!(!b.redo());
}

#[test]
fn new_edit_after_undo_invalidates_redo_stack() {
    let mut b = buf("");
    for ch in "abc".chars() {
        b.insert_char(ch);
    }
    b.exit_insert_mode();
    b.undo();
    assert_eq!(b.content_as_string(), "");
    b.insert_char('Z');
    b.exit_insert_mode();
    // The "abc" branch was abandoned; redo must not bring it back.
    assert!(!b.redo());
    assert_eq!(b.content_as_string(), "Z");
}

#[test]
fn backspace_run_undoes_as_one_chunk() {
    let mut b = buf("hello");
    b.cursor.col = 5;
    b.backspace();
    b.backspace();
    b.backspace();
    b.exit_insert_mode();
    assert_eq!(b.content_as_string(), "he");
    assert!(b.undo());
    assert_eq!(b.content_as_string(), "hello");
}

#[test]
fn forward_delete_run_undoes_as_one_chunk() {
    let mut b = buf("hello");
    b.cursor.col = 0;
    b.delete_forward();
    b.delete_forward();
    b.exit_insert_mode();
    assert_eq!(b.content_as_string(), "llo");
    assert!(b.undo());
    assert_eq!(b.content_as_string(), "hello");
}

#[test]
fn delete_then_insert_are_separate_undo_steps() {
    let mut b = buf("hello");
    b.cursor.col = 5;
    b.backspace();
    b.insert_char('Y');
    b.exit_insert_mode();
    assert!(b.undo());
    assert_eq!(b.content_as_string(), "hell");
    assert!(b.undo());
    assert_eq!(b.content_as_string(), "hello");
}

#[test]
fn newline_seals_the_prior_insert_run() {
    let mut b = buf("");
    for ch in "foo".chars() {
        b.insert_char(ch);
    }
    b.insert_newline();
    for ch in "bar".chars() {
        b.insert_char(ch);
    }
    b.exit_insert_mode();
    assert_eq!(b.content_as_string(), "foo\nbar");
    assert!(b.undo());
    assert_eq!(b.content_as_string(), "foo\n");
    assert!(b.undo());
    assert_eq!(b.content_as_string(), "foo");
    assert!(b.undo());
    assert_eq!(b.content_as_string(), "");
}

#[test]
fn paste_is_a_single_undo_step() {
    let mut b = buf("a\n");
    b.cursor.line = 0;
    b.cursor.col = 0;
    b.paste_line_below("b\nc\n");
    b.exit_insert_mode();
    assert!(b.content_as_string().contains('b'));
    assert!(b.undo());
    assert_eq!(b.content_as_string(), "a\n");
}

#[test]
fn explicit_begin_end_group_collapses_multiple_edits() {
    let mut b = buf("");
    b.begin_undo_group();
    b.insert_char('a');
    b.move_word_forward();
    b.insert_char('b');
    b.end_undo_group();
    b.exit_insert_mode();
    assert_eq!(b.content_as_string(), "ab");
    assert!(b.undo());
    assert_eq!(b.content_as_string(), "");
}

#[test]
fn block_indent_undoes_as_one_step() {
    let mut b = buf("a\nb\nc\n");
    b.cursor.line = 0;
    b.cursor.col = 0;
    b.set_selection_anchor();
    b.cursor.line = 2;
    b.indent_selection();
    let after = b.content_as_string();
    assert!(after.starts_with("    a"));
    assert!(b.undo());
    assert_eq!(b.content_as_string(), "a\nb\nc\n");
}

#[test]
fn multi_buffer_history_is_isolated() {
    let mut a = buf("a");
    let mut b = buf("b");
    a.cursor.col = 1;
    b.cursor.col = 1;
    a.insert_char('X');
    a.exit_insert_mode();
    // No edits in `b` — its undo stack must be empty regardless of `a`.
    assert!(!b.undo());
    // Undoing `a` must not consult `b`.
    assert!(a.undo());
    assert_eq!(a.content_as_string(), "a");
}

#[test]
fn auto_pair_insert_undoes_atomically() {
    let mut b = buf("");
    b.insert_pair('(', ')');
    b.exit_insert_mode();
    assert_eq!(b.content_as_string(), "()");
    assert!(b.undo());
    assert_eq!(b.content_as_string(), "");
}

#[test]
fn visual_selection_replacement_is_one_transaction() {
    let mut b = buf("hello");
    b.cursor.col = 0;
    b.set_selection_anchor();
    b.cursor.col = 4;
    b.replace_selection_with_str("XX");
    b.exit_insert_mode();
    assert_eq!(b.content_as_string(), "XX");
    assert!(b.undo());
    assert_eq!(b.content_as_string(), "hello");
}

#[test]
fn save_marks_history_anchor_for_dirty_relative() {
    let mut b = buf("");
    b.insert_char('a');
    b.exit_insert_mode();
    assert!(b.is_dirty());
    b.mark_clean();
    assert!(!b.is_dirty_relative_to_saved());
    b.insert_char('b');
    assert!(b.is_dirty_relative_to_saved());
    b.undo();
    assert!(!b.is_dirty_relative_to_saved());
}

#[test]
fn long_session_does_not_lose_recent_history() {
    let mut b = buf("");
    for i in 0..50 {
        b.insert_char((b'a' + (i % 26) as u8) as char);
        b.move_word_forward();
    }
    b.exit_insert_mode();
    // We should be able to undo at least the last ten distinct groups.
    for _ in 0..10 {
        assert!(b.undo(), "undo ran out before exhausting recent history");
    }
}
