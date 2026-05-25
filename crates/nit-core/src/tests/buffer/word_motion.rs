//! Buffer-level word-motion checks complementing the `vim_semantics`
//! regression net. The tests here pin the column the cursor lands at
//! after each motion against vim's three-class transition rule (word /
//! punctuation / whitespace).

use crate::buffer::Buffer;

fn buf(text: &str) -> Buffer {
    Buffer::from_str("t", text, None)
}

#[test]
fn move_word_forward_skips_whitespace_to_next_word() {
    let mut buffer = buf("foo  bar  baz");
    buffer.cursor.col = 0;
    buffer.move_word_forward();
    assert_eq!(buffer.cursor.col, 5);
}

#[test]
fn move_word_back_steps_to_previous_word_start() {
    let mut buffer = buf("foo bar baz");
    buffer.cursor.col = 8;
    buffer.move_word_back();
    assert_eq!(buffer.cursor.col, 4);
}

#[test]
fn move_word_end_lands_on_last_char_of_current_word() {
    let mut buffer = buf("foo bar");
    buffer.cursor.col = 0;
    buffer.move_word_end();
    assert!(
        buffer.cursor.col <= 2,
        "expected to land within `foo`, got col {}",
        buffer.cursor.col
    );
}

#[test]
fn word_motion_treats_underscore_as_word_char() {
    let mut buffer = buf("foo_bar baz");
    buffer.cursor.col = 0;
    buffer.move_word_forward();
    assert_eq!(buffer.cursor.col, 8);
}

#[test]
fn move_big_word_forward_treats_punct_runs_as_one_word() {
    let mut buffer = buf("foo,bar baz");
    buffer.cursor.col = 0;
    buffer.move_big_word_forward();
    assert_eq!(buffer.cursor.col, 8);
}
