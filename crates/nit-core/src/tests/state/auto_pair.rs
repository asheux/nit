use crate::actions::Action;
use crate::buffer::Buffer;
use crate::mode::Mode;
use crate::state::action_apply::apply_action;
use crate::state::AppState;
use crate::test_helpers::temp_dir;

fn state_with(text: &str) -> AppState {
    let root = temp_dir("auto_pair");
    let mut state = AppState::new(
        root,
        Buffer::from_str("x", text, None),
        Buffer::empty("n", None),
    );
    state.mode = Mode::Insert;
    state
}

fn text(state: &AppState) -> String {
    state.editor_buffer().content_as_string()
}

fn type_chars(state: &mut AppState, chars: &str) {
    for c in chars.chars() {
        apply_action(state, Action::InsertChar(c));
    }
}

#[test]
fn paren_auto_closes() {
    let mut state = state_with("");
    type_chars(&mut state, "(");
    assert_eq!(text(&state), "()");
    assert_eq!(state.editor_buffer().cursor.col, 1);
}

#[test]
fn bracket_auto_closes() {
    let mut state = state_with("");
    type_chars(&mut state, "[");
    assert_eq!(text(&state), "[]");
}

#[test]
fn brace_auto_closes() {
    let mut state = state_with("");
    type_chars(&mut state, "{");
    assert_eq!(text(&state), "{}");
}

#[test]
fn double_quote_auto_closes() {
    let mut state = state_with("");
    type_chars(&mut state, "\"");
    assert_eq!(text(&state), "\"\"");
    assert_eq!(state.editor_buffer().cursor.col, 1);
}

#[test]
fn single_quote_auto_closes() {
    let mut state = state_with("");
    type_chars(&mut state, "'");
    assert_eq!(text(&state), "''");
}

#[test]
fn typing_close_at_existing_close_skips_over() {
    // Type `()`: the `(` auto-pairs to `()`, the explicit `)` then meets the
    // existing `)` at the cursor and just moves past it instead of inserting.
    let mut state = state_with("");
    type_chars(&mut state, "()");
    assert_eq!(text(&state), "()");
    assert_eq!(state.editor_buffer().cursor.col, 2);
}

#[test]
fn no_pair_before_word_char() {
    // Wrapping existing text: `(foo` shouldn't insert `)` after the `(`,
    // since the user is likely typing `(foo)` around an existing identifier.
    let mut state = state_with("foo");
    // Cursor starts at (0,0); `(` should NOT auto-pair because next char is 'f'.
    type_chars(&mut state, "(");
    assert_eq!(text(&state), "(foo");
}

#[test]
fn no_quote_pair_inside_word() {
    // Apostrophe inside an identifier (e.g. `don't`) should be a literal char.
    let mut state = state_with("don");
    state.editor_buffer_mut().cursor.col = 3;
    type_chars(&mut state, "'");
    assert_eq!(text(&state), "don'");
}

#[test]
fn pair_before_punctuation_ok() {
    // Auto-pair when the next char is not alphanumeric.
    let mut state = state_with(";");
    type_chars(&mut state, "(");
    assert_eq!(text(&state), "();");
    assert_eq!(state.editor_buffer().cursor.col, 1);
}

#[test]
fn non_pair_char_unaffected() {
    let mut state = state_with("");
    type_chars(&mut state, "abc");
    assert_eq!(text(&state), "abc");
}
