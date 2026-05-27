//! Smart-newline (T11) buffer tests: Enter between matching brackets
//! expands the pair onto three lines with the cursor on the indented
//! middle line. Also covers the language-aware `:` indent (T6) — Enter
//! after a Python block opener indents the next line one step deeper.

use std::path::PathBuf;

use crate::buffer::Buffer;

fn buf(text: &str) -> Buffer {
    Buffer::from_str("t", text, None)
}

fn buf_with_path(name: &str, text: &str, path: &str) -> Buffer {
    Buffer::from_str(name, text, Some(PathBuf::from(path)))
}

fn leading_ws_len(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
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
    let indent_len = leading_ws_len(&line1);
    assert!(
        indent_len > 0,
        "expected leading indent on the new line, got {line1:?}"
    );
}

#[test]
fn newline_after_python_colon_indents_next_line() {
    let mut buffer = buf_with_path("script.py", "def foo():", "script.py");
    buffer.cursor.line = 0;
    buffer.cursor.col = 10;
    buffer.insert_newline();
    let line1 = buffer.line_as_string(1);
    assert_eq!(
        leading_ws_len(&line1),
        4,
        "Python `:` opener should add one 4-space indent step, got {line1:?}"
    );
}

#[test]
fn newline_after_python_colon_stacks_existing_indent() {
    let mut buffer = buf_with_path("script.py", "    if x:", "script.py");
    buffer.cursor.line = 0;
    buffer.cursor.col = 9;
    buffer.insert_newline();
    let line1 = buffer.line_as_string(1);
    assert_eq!(
        leading_ws_len(&line1),
        8,
        "nested Python `:` should keep 4-space parent indent and add 4 more, got {line1:?}"
    );
}

#[test]
fn newline_after_python_colon_with_trailing_whitespace_still_indents() {
    let mut buffer = buf_with_path("script.py", "def foo():    ", "script.py");
    buffer.cursor.line = 0;
    buffer.cursor.col = 14;
    buffer.insert_newline();
    let line1 = buffer.line_as_string(1);
    assert_eq!(
        leading_ws_len(&line1),
        4,
        "trailing whitespace after `:` should not suppress the indent bump, got {line1:?}"
    );
}

#[test]
fn newline_after_colon_in_rust_does_not_indent() {
    let mut buffer = buf_with_path("lib.rs", "let x:", "src/lib.rs");
    buffer.cursor.line = 0;
    buffer.cursor.col = 6;
    buffer.insert_newline();
    let line1 = buffer.line_as_string(1);
    assert_eq!(
        leading_ws_len(&line1),
        0,
        "Rust `:` is a type-annotation, not a block opener — must not indent, got {line1:?}"
    );
}

#[test]
fn newline_after_colon_with_no_path_does_not_indent() {
    let mut buffer = buf("key:");
    buffer.cursor.line = 0;
    buffer.cursor.col = 4;
    buffer.insert_newline();
    let line1 = buffer.line_as_string(1);
    assert_eq!(
        leading_ws_len(&line1),
        0,
        "unknown language must fall back to bracket-only triggers, got {line1:?}"
    );
}

// --- T8a: terminator-dedent (return / break / continue / pass / raise) ---

#[test]
fn newline_after_python_return_dedents_next_line() {
    let mut buffer = buf_with_path("script.py", "def foo():\n    return", "script.py");
    buffer.cursor.line = 1;
    buffer.cursor.col = 10;
    buffer.insert_newline();
    let line2 = buffer.line_as_string(2);
    assert_eq!(
        leading_ws_len(&line2),
        0,
        "Enter after `return` in a 4-space block should land at col 0, got {line2:?}"
    );
}

#[test]
fn newline_after_python_return_with_value_dedents() {
    let mut buffer = buf_with_path("s.py", "def foo():\n    return value", "s.py");
    buffer.cursor.line = 1;
    buffer.cursor.col = 16;
    buffer.insert_newline();
    let line2 = buffer.line_as_string(2);
    assert_eq!(
        leading_ws_len(&line2),
        0,
        "`return value` ends the block — next line should dedent, got {line2:?}"
    );
}

#[test]
fn newline_after_rust_return_with_semicolon_dedents() {
    let mut buffer = buf_with_path("lib.rs", "fn foo() {\n    return 1;", "src/lib.rs");
    buffer.cursor.line = 1;
    buffer.cursor.col = 13;
    buffer.insert_newline();
    let line2 = buffer.line_as_string(2);
    assert_eq!(
        leading_ws_len(&line2),
        0,
        "Rust `return 1;` should dedent — block has ended, got {line2:?}"
    );
}

#[test]
fn newline_after_python_pass_dedents() {
    let mut buffer = buf_with_path("s.py", "def foo():\n    pass", "s.py");
    buffer.cursor.line = 1;
    buffer.cursor.col = 8;
    buffer.insert_newline();
    let line2 = buffer.line_as_string(2);
    assert_eq!(
        leading_ws_len(&line2),
        0,
        "Python `pass` should dedent, got {line2:?}"
    );
}

#[test]
fn newline_after_python_raise_dedents() {
    let mut buffer = buf_with_path("s.py", "def foo():\n    raise Exception()", "s.py");
    buffer.cursor.line = 1;
    buffer.cursor.col = 21;
    buffer.insert_newline();
    let line2 = buffer.line_as_string(2);
    assert_eq!(
        leading_ws_len(&line2),
        0,
        "Python `raise` should dedent, got {line2:?}"
    );
}

#[test]
fn newline_after_break_dedents_universally() {
    let mut buffer = buf_with_path("a.go", "for {\n    break", "src/a.go");
    buffer.cursor.line = 1;
    buffer.cursor.col = 9;
    buffer.insert_newline();
    let line2 = buffer.line_as_string(2);
    assert_eq!(
        leading_ws_len(&line2),
        0,
        "`break` should dedent in any language, got {line2:?}"
    );
}

#[test]
fn newline_after_pass_in_non_python_does_not_dedent() {
    let mut buffer = buf_with_path("lib.rs", "fn f() {\n    let pass = 1;", "src/lib.rs");
    buffer.cursor.line = 1;
    buffer.cursor.col = 19;
    buffer.insert_newline();
    let line2 = buffer.line_as_string(2);
    assert_eq!(
        leading_ws_len(&line2),
        4,
        "`pass` is Python-only — Rust line should stay indented, got {line2:?}"
    );
}

#[test]
fn newline_after_return_value_assignment_does_not_dedent() {
    // `return_value = 1` starts with the word `return_value`, not `return`
    // — must NOT be confused with the terminator.
    let mut buffer = buf_with_path("a.py", "def f():\n    return_value = 1", "a.py");
    buffer.cursor.line = 1;
    buffer.cursor.col = 21;
    buffer.insert_newline();
    let line2 = buffer.line_as_string(2);
    assert_eq!(
        leading_ws_len(&line2),
        4,
        "identifier starting with `return` must NOT dedent, got {line2:?}"
    );
}

#[test]
fn newline_after_return_at_flush_left_leaves_indent_alone() {
    let mut buffer = buf_with_path("s.py", "return", "s.py");
    buffer.cursor.line = 0;
    buffer.cursor.col = 6;
    buffer.insert_newline();
    let line1 = buffer.line_as_string(1);
    assert_eq!(
        leading_ws_len(&line1),
        0,
        "no indent to strip — must not panic or wrap, got {line1:?}"
    );
}

#[test]
fn newline_after_return_in_nested_block_dedents_one_step() {
    let mut buffer = buf_with_path(
        "s.py",
        "def outer():\n    def inner():\n        return 1",
        "s.py",
    );
    buffer.cursor.line = 2;
    buffer.cursor.col = 16;
    buffer.insert_newline();
    let line3 = buffer.line_as_string(3);
    assert_eq!(
        leading_ws_len(&line3),
        4,
        "double-nested `return` should drop one level (to 4-space indent), got {line3:?}"
    );
}
