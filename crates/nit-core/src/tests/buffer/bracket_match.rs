use super::*;

fn buf_at(text: &str, line: usize, col: usize) -> Buffer {
    let mut buf = Buffer::from_str("test", text, None);
    buf.cursor.line = line;
    buf.cursor.col = col;
    buf
}

#[test]
fn paren_open_jumps_to_close() {
    let mut buf = buf_at("foo(bar)", 0, 3);
    buf.match_bracket();
    assert_eq!((buf.cursor.line, buf.cursor.col), (0, 7));
}

#[test]
fn paren_close_jumps_to_open() {
    let mut buf = buf_at("foo(bar)", 0, 7);
    buf.match_bracket();
    assert_eq!((buf.cursor.line, buf.cursor.col), (0, 3));
}

#[test]
fn square_brackets_match() {
    let mut buf = buf_at("arr[0]", 0, 3);
    buf.match_bracket();
    assert_eq!(buf.cursor.col, 5);

    buf.match_bracket();
    assert_eq!(buf.cursor.col, 3);
}

#[test]
fn curly_braces_match() {
    let mut buf = buf_at("fn x() {body}", 0, 7);
    buf.match_bracket();
    assert_eq!(buf.cursor.col, 12);
}

#[test]
fn angle_brackets_match() {
    let mut buf = buf_at("Vec<u32>", 0, 3);
    buf.match_bracket();
    assert_eq!(buf.cursor.col, 7);

    buf.match_bracket();
    assert_eq!(buf.cursor.col, 3);
}

#[test]
fn nested_parens_resolve_outer_pair() {
    let mut buf = buf_at("((inner))", 0, 0);
    buf.match_bracket();
    assert_eq!(buf.cursor.col, 8);

    let mut buf = buf_at("((inner))", 0, 1);
    buf.match_bracket();
    assert_eq!(buf.cursor.col, 7);
}

#[test]
fn nested_mixed_pairs_ignore_other_kinds() {
    let mut buf = buf_at("([{}])", 0, 0);
    buf.match_bracket();
    assert_eq!(buf.cursor.col, 5);

    let mut buf = buf_at("([{}])", 0, 1);
    buf.match_bracket();
    assert_eq!(buf.cursor.col, 4);
}

#[test]
fn matches_across_multiple_lines() {
    let mut buf = buf_at("fn x() {\n    body();\n}", 0, 7);
    buf.match_bracket();
    assert_eq!((buf.cursor.line, buf.cursor.col), (2, 0));
}

#[test]
fn cursor_not_on_bracket_scans_forward_on_line() {
    let mut buf = buf_at("let xs = vec![1, 2];", 0, 0);
    buf.match_bracket();
    assert_eq!((buf.cursor.line, buf.cursor.col), (0, 18));
}

#[test]
fn no_match_leaves_cursor_in_place() {
    let mut buf = buf_at("unbalanced(", 0, 10);
    buf.match_bracket();
    assert_eq!(buf.cursor.col, 10);
}

#[test]
fn no_bracket_on_line_is_noop() {
    let mut buf = buf_at("plain text only", 0, 4);
    buf.match_bracket();
    assert_eq!(buf.cursor.col, 4);
}
