use super::*;

#[test]
fn undo_single_step_on_selection_replace_char() {
    let mut buf = Buffer::from_str("test", "hello", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.set_selection_anchor();
    buf.move_end();

    buf.insert_char('x');
    assert_eq!(buf.content_as_string(), "x");

    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "hello");

    assert!(buf.redo());
    assert_eq!(buf.content_as_string(), "x");
}

#[test]
fn undo_single_step_on_selection_replace_str() {
    let mut buf = Buffer::from_str("test", "hello world", None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.set_selection_anchor();
    buf.move_end();

    buf.insert_str("XYZ");
    assert_eq!(buf.content_as_string(), "XYZ");

    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "hello world");
}

#[test]
fn undo_group_breaks_on_cursor_move() {
    let mut buf = Buffer::empty("test", None);

    buf.insert_char('a');
    buf.move_left();
    buf.move_right();
    buf.insert_char('b');
    assert_eq!(buf.content_as_string(), "ab");

    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "a");

    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "");
}

#[test]
fn break_undo_group_splits_inserts() {
    let mut buf = Buffer::empty("test", None);

    buf.insert_char('a');
    buf.break_undo_group();
    buf.insert_str("b");
    assert_eq!(buf.content_as_string(), "ab");

    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "a");
}

#[test]
fn undo_single_step_on_selection_replace_newline_preserves_indent() {
    let mut buf = Buffer::from_str("test", "    foo", None);
    buf.cursor.line = 0;
    buf.cursor.col = 4;
    buf.set_selection_anchor();
    buf.move_end();

    buf.insert_newline();
    assert_eq!(buf.content_as_string(), "    \n    ");
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 4);

    assert!(buf.undo());
    assert_eq!(buf.content_as_string(), "    foo");
}
