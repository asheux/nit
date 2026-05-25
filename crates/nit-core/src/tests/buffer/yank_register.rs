//! Buffer-level half of the yank-register contract.
//!
//! The state-layer round-trip tests for `Action::YankLine` / `Paste` live
//! in `tests/state/yank_register.rs`. Tests in this file isolate the
//! buffer-side primitives (`yank_line`, `yank_selection`,
//! `paste_line_above`, `paste_line_below`, `delete_line`, `delete_to_end`)
//! that the action layer composes — so a regression in the buffer surface
//! shows up here, decoupled from the action-routing logic.

use crate::buffer::Buffer;

fn buf(text: &str) -> Buffer {
    Buffer::from_str("t", text, None)
}

#[test]
fn yank_line_appends_trailing_newline_when_missing() {
    let mut buffer = buf("alpha");
    buffer.cursor.line = 0;
    buffer.cursor.col = 0;
    let yanked = buffer.yank_line();
    assert_eq!(yanked, "alpha\n");
}

#[test]
fn yank_line_keeps_existing_newline() {
    let mut buffer = buf("alpha\nbeta\n");
    buffer.cursor.line = 0;
    buffer.cursor.col = 0;
    assert_eq!(buffer.yank_line(), "alpha\n");
}

#[test]
fn paste_line_below_inserts_after_current_line() {
    let mut buffer = buf("first\nsecond\n");
    buffer.cursor.line = 0;
    buffer.cursor.col = 0;
    buffer.paste_line_below("payload\n");
    assert_eq!(buffer.content_as_string(), "first\npayload\nsecond\n");
    assert_eq!(buffer.cursor.line, 1);
}

#[test]
fn paste_line_above_inserts_before_current_line() {
    let mut buffer = buf("first\nsecond\n");
    buffer.cursor.line = 1;
    buffer.cursor.col = 0;
    buffer.paste_line_above("payload\n");
    assert_eq!(buffer.content_as_string(), "first\npayload\nsecond\n");
}

#[test]
fn yank_selection_returns_selected_slice() {
    // Visual char-wise selection is inclusive of both endpoints — the
    // cursor's char is part of the slice.
    let mut buffer = buf("foobar\n");
    buffer.cursor.line = 0;
    buffer.cursor.col = 1;
    buffer.set_selection_anchor();
    buffer.cursor.col = 4;
    let yanked = buffer.yank_selection();
    assert_eq!(yanked, Some("ooba".into()));
}

#[test]
fn delete_line_removes_line_content() {
    let mut buffer = buf("alpha\nbeta\ngamma\n");
    buffer.cursor.line = 1;
    buffer.cursor.col = 0;
    buffer.delete_line();
    assert_eq!(buffer.content_as_string(), "alpha\ngamma\n");
}

#[test]
fn delete_to_end_trims_to_eol() {
    let mut buffer = buf("hello world\n");
    buffer.cursor.line = 0;
    buffer.cursor.col = 5;
    buffer.delete_to_end();
    assert_eq!(buffer.content_as_string(), "hello\n");
}
