//! Visual-mode `U`/`u` case conversion: Unicode-correct folding of the
//! selection via `Buffer::uppercase_selection`/`lowercase_selection`, covering
//! length-changing folds (ß→SS, Greek final sigma), cursor landing, and the
//! single-undo guarantee. Each test selects an inclusive char span on line 0
//! (anchor at the start, cursor on the last cell — the shape `v` + motion
//! produces) and asserts the rewritten buffer.

use crate::buffer::Buffer;

#[test]
fn uppercases_ascii_selection() {
    let mut sentence = Buffer::from_str("t", "hello world", None);
    sentence.cursor.col = 0;
    sentence.set_selection_anchor();
    sentence.cursor.col = 4;
    sentence.uppercase_selection();
    assert_eq!(sentence.content_as_string(), "HELLO world");
}

#[test]
fn lowercases_ascii_selection() {
    let mut shout = Buffer::from_str("t", "HELLO WORLD", None);
    shout.cursor.col = 0;
    shout.set_selection_anchor();
    shout.cursor.col = 10;
    shout.lowercase_selection();
    assert_eq!(shout.content_as_string(), "hello world");
}

#[test]
fn uppercases_mixed_case_run() {
    let mut camel = Buffer::from_str("t", "HeLLo", None);
    camel.cursor.col = 0;
    camel.set_selection_anchor();
    camel.cursor.col = 4;
    camel.uppercase_selection();
    assert_eq!(camel.content_as_string(), "HELLO");
}

#[test]
fn lowercases_mixed_case_run() {
    let mut jumbled = Buffer::from_str("t", "HeLLo", None);
    jumbled.cursor.col = 0;
    jumbled.set_selection_anchor();
    jumbled.cursor.col = 4;
    jumbled.lowercase_selection();
    assert_eq!(jumbled.content_as_string(), "hello");
}

#[test]
fn sharp_s_uppercases_to_double_s() {
    // ß has no single-codepoint uppercase; str::to_uppercase yields "SS",
    // growing the buffer where byte-level ASCII never could.
    let mut street = Buffer::from_str("t", "straße", None);
    street.cursor.col = 0;
    street.set_selection_anchor();
    street.cursor.col = 5;
    street.uppercase_selection();
    assert_eq!(street.content_as_string(), "STRASSE");
}

#[test]
fn single_sharp_s_grows_the_line() {
    let mut glyph = Buffer::from_str("t", "ßx", None);
    glyph.cursor.col = 0;
    glyph.set_selection_anchor();
    glyph.cursor.col = 0;
    glyph.uppercase_selection();
    assert_eq!(glyph.content_as_string(), "SSx");
}

#[test]
fn greek_sigma_uppercases_to_capital() {
    let mut noun = Buffer::from_str("t", "ος", None);
    noun.cursor.col = 0;
    noun.set_selection_anchor();
    noun.cursor.col = 1;
    noun.uppercase_selection();
    assert_eq!(noun.content_as_string(), "ΟΣ");
}

#[test]
fn word_final_sigma_lowercases_to_final_form() {
    // str::to_lowercase honours Unicode's context-sensitive rule: a word-final
    // Σ folds to the final form ς (U+03C2), not the medial σ.
    let mut title = Buffer::from_str("t", "ΟΣ", None);
    title.cursor.col = 0;
    title.set_selection_anchor();
    title.cursor.col = 1;
    title.lowercase_selection();
    assert_eq!(title.content_as_string(), "ος");
}

#[test]
fn cursor_returns_to_selection_start() {
    let mut greeting = Buffer::from_str("t", "hello", None);
    greeting.cursor.col = 1;
    greeting.set_selection_anchor();
    greeting.cursor.col = 4;
    greeting.uppercase_selection();
    assert_eq!(greeting.content_as_string(), "hELLO");
    assert_eq!(greeting.cursor.col, 1);
}

#[test]
fn cursor_holds_start_when_fold_grows_line() {
    // ß→SS lengthens the text, yet the cursor still pins to the start column.
    let mut padded = Buffer::from_str("t", "aßb", None);
    padded.cursor.col = 1;
    padded.set_selection_anchor();
    padded.cursor.col = 1;
    padded.uppercase_selection();
    assert_eq!(padded.content_as_string(), "aSSb");
    assert_eq!(padded.cursor.col, 1);
}

#[test]
fn single_undo_rewinds_plain_selection() {
    let mut phrase = Buffer::from_str("t", "Hello World", None);
    phrase.cursor.col = 0;
    phrase.set_selection_anchor();
    phrase.cursor.col = 10;
    phrase.uppercase_selection();
    assert_eq!(phrase.content_as_string(), "HELLO WORLD");
    assert!(phrase.undo());
    assert_eq!(phrase.content_as_string(), "Hello World");
}

#[test]
fn single_undo_rewinds_length_changing_fold() {
    let mut german = Buffer::from_str("t", "straße", None);
    german.cursor.col = 0;
    german.set_selection_anchor();
    german.cursor.col = 5;
    german.uppercase_selection();
    assert_eq!(german.content_as_string(), "STRASSE");
    assert!(german.undo());
    assert_eq!(german.content_as_string(), "straße");
}
