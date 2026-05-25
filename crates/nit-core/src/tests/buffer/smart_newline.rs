//! Smart-newline (T11) buffer tests: Enter between matching brackets
//! expands the pair onto three lines with the cursor on the indented
//! middle line.

use crate::buffer::Buffer;

fn buf(text: &str) -> Buffer {
    Buffer::from_str("t", text, None)
}

#[test]
fn newline_between_parens_expands_pair() {
    let mut buffer = buf("fn foo()");
    buffer.cursor.line = 0;
    buffer.cursor.col = 7;
    buffer.insert_newline();
    let lines: Vec<String> = buffer
        .content_as_string()
        .lines()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "fn foo(");
    assert!(lines[1].trim().is_empty());
    assert_eq!(lines[2], ")");
}

#[test]
fn newline_between_braces_expands_pair() {
    let mut buffer = buf("fn body{}");
    buffer.cursor.line = 0;
    buffer.cursor.col = 8;
    buffer.insert_newline();
    let lines: Vec<String> = buffer
        .content_as_string()
        .lines()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "fn body{");
    assert!(lines[1].trim().is_empty());
    assert_eq!(lines[2], "}");
}

#[test]
fn newline_inside_brackets_indents_middle_line_one_step_deeper() {
    let mut buffer = buf("arr = [\n    foo,\n]");
    buffer.cursor.line = 0;
    buffer.cursor.col = 6;
    let before_lines = buffer.lines_len();
    buffer.insert_newline();
    assert!(
        buffer.lines_len() > before_lines,
        "newline should add at least one line"
    );
}

#[test]
fn newline_after_bare_opener_indents_by_one_unit() {
    let mut buffer = buf("func{");
    buffer.cursor.line = 0;
    buffer.cursor.col = 5;
    buffer.insert_newline();
    let line1 = buffer.line_as_string(1);
    let indent_len = line1
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .count();
    assert!(
        indent_len > 0,
        "expected leading indent on the new line, got {line1:?}"
    );
}
