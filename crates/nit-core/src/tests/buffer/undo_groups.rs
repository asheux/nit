//! Undo-group boundary checks.
//!
//! Vim's undo unit is an "edit chunk": one insert run, one delete, one
//! paste. Motion / mode-change / paste / newline-insert each break the
//! current group. These tests pin the boundaries the buffer enforces so
//! a regression in `begin_insert_group` / `finish_insert_group` /
//! `break_undo_group` doesn't silently flip behaviour back to
//! character-at-a-time undo.

use crate::buffer::Buffer;

fn buf(text: &str) -> Buffer {
    Buffer::from_str("t", text, None)
}

#[test]
fn one_undo_rewinds_a_full_insert_run() {
    let mut buffer = buf("");
    for ch in "hello".chars() {
        buffer.insert_char(ch);
    }
    buffer.exit_insert_mode();
    assert_eq!(buffer.content_as_string(), "hello");
    assert!(buffer.undo());
    assert_eq!(buffer.content_as_string(), "");
}

#[test]
fn motion_between_inserts_splits_undo_groups() {
    let mut buffer = buf("hello world");
    buffer.cursor.col = 0;
    buffer.insert_char('X');
    buffer.move_word_forward();
    buffer.insert_char('Y');
    buffer.exit_insert_mode();
    assert!(buffer.undo());
    let after_first = buffer.content_as_string();
    assert!(!after_first.contains('Y'));
    assert!(buffer.undo());
    assert!(!buffer.content_as_string().contains('X'));
}

#[test]
fn delete_then_insert_are_separate_undo_steps() {
    let mut buffer = buf("hello");
    buffer.cursor.col = 5;
    buffer.backspace();
    buffer.insert_char('Y');
    buffer.exit_insert_mode();
    assert!(buffer.undo());
    assert_eq!(buffer.content_as_string(), "hell");
    assert!(buffer.undo());
    assert_eq!(buffer.content_as_string(), "hello");
}

#[test]
fn redo_replays_the_last_undone_chunk() {
    let mut buffer = buf("");
    for ch in "abc".chars() {
        buffer.insert_char(ch);
    }
    buffer.exit_insert_mode();
    buffer.undo();
    assert_eq!(buffer.content_as_string(), "");
    buffer.redo();
    assert_eq!(buffer.content_as_string(), "abc");
}
