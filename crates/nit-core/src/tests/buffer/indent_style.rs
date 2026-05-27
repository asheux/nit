//! Buffer-level indent-style detection.
//!
//! `Buffer::indent_unit` is private to the buffer module, so these tests
//! exercise it indirectly via the public surface that depends on it:
//! `insert_newline` (which copies the leading indent of the current line)
//! and `open_line_below` (which adds one indent step when the previous
//! line ends with a block opener).

use crate::buffer::Buffer;

fn buf(text: &str) -> Buffer {
    Buffer::from_str("t", text, None)
}

#[test]
fn newline_preserves_4_space_indent() {
    let mut buffer = buf("def foo():\n    pass\n");
    buffer.cursor.line = 1;
    buffer.cursor.col = 8;
    buffer.insert_newline();
    let line2 = buffer.line_as_string(2);
    let indent_len = line2.chars().take_while(|c| *c == ' ').count();
    assert_eq!(
        indent_len, 4,
        "expected 4-space indent on new line, got {line2:?}"
    );
}

#[test]
fn newline_preserves_2_space_indent() {
    // `let x = 1;` is intentionally NOT a control-flow terminator — the
    // T8a smart-dedent rule fires after `return` / `break` / `continue`,
    // so this test exercises pure indent-unit detection without bumping
    // into the dedent path.
    let mut buffer = buf("function foo() {\n  let x = 1;\n}\n");
    buffer.cursor.line = 1;
    buffer.cursor.col = 12;
    buffer.insert_newline();
    let line2 = buffer.line_as_string(2);
    let indent_len = line2.chars().take_while(|c| *c == ' ').count();
    assert_eq!(indent_len, 2);
}

#[test]
fn newline_preserves_tab_indent() {
    let mut buffer = buf("func foo() {\n\tlet x = 1\n}\n");
    buffer.cursor.line = 1;
    buffer.cursor.col = 10;
    buffer.insert_newline();
    let line2 = buffer.line_as_string(2);
    assert!(
        line2.starts_with('\t'),
        "expected tab indent, got {line2:?}"
    );
}

#[test]
fn open_line_below_after_colon_adds_one_indent_step() {
    // Python-style: line ends with `:` so `o` should add one indent step.
    // The `:`-as-block-opener rule is language-gated (see is_block_starter),
    // so the buffer is tagged with a `.py` path to activate the Python branch.
    let mut buffer = Buffer::from_str(
        "script.py",
        "def foo():\n    pass\n",
        Some(std::path::PathBuf::from("script.py")),
    );
    buffer.cursor.line = 0;
    buffer.cursor.col = 10;
    buffer.open_line_below();
    let line1 = buffer.line_as_string(1);
    let indent_len = line1.chars().take_while(|c| *c == ' ').count();
    assert!(
        indent_len >= 4,
        "expected ≥4-space indent after `:`, got {line1:?}"
    );
}

#[test]
fn open_line_below_after_brace_adds_one_indent_step() {
    let mut buffer = buf("function foo() {\n  return 1;\n}\n");
    buffer.cursor.line = 0;
    buffer.cursor.col = 16;
    buffer.open_line_below();
    let line1 = buffer.line_as_string(1);
    let indent_len = line1.chars().take_while(|c| *c == ' ').count();
    assert!(
        indent_len >= 2,
        "expected ≥2-space indent after `{{`, got {line1:?}"
    );
}
