; Rust highlights (minimal, editor-grade)
(line_comment) @comment
(block_comment) @comment

(string_literal) @string
(raw_string_literal) @string
(char_literal) @character

(integer_literal) @number
(float_literal) @number

["true" "false"] @boolean

[
  "fn"
  "let"
  "mut"
  "pub"
  "use"
  "impl"
  "trait"
  "struct"
  "enum"
  "match"
  "if"
  "else"
  "for"
  "while"
  "loop"
  "break"
  "continue"
  "return"
  "async"
  "await"
  "unsafe"
  "const"
  "static"
  "where"
  "type"
  "mod"
  "crate"
  "super"
  "self"
] @keyword

(primitive_type) @type.builtin
(type_identifier) @type

(function_item name: (identifier) @function)
(macro_invocation macro: (identifier) @macro)

(attribute_item) @attribute
(inner_attribute_item) @attribute

((identifier) @constant (#match? @constant "^[A-Z_][A-Z0-9_]+$"))
