; Rust highlights (editor-grade, full token taxonomy)
(line_comment) @comment
(block_comment) @comment

(string_literal) @string
(raw_string_literal) @string
(escape_sequence) @escape
(char_literal) @character

(integer_literal) @number
(float_literal) @number

["true" "false"] @boolean

; Control-flow keywords get their own group so they read distinctly.
[
  "if"
  "else"
  "match"
  "for"
  "while"
  "loop"
  "break"
  "continue"
  "return"
  "await"
  "yield"
] @keyword.control

[
  "fn"
  "let"
  "pub"
  "use"
  "impl"
  "trait"
  "struct"
  "enum"
  "async"
  "unsafe"
  "const"
  "static"
  "where"
  "type"
  "mod"
  "as"
  "in"
  "move"
  "ref"
  "dyn"
  "extern"
  "default"
  "union"
] @keyword

; Named nodes that are keywords in tree-sitter-rust.
(mutable_specifier) @keyword
(self) @keyword
(super) @keyword
(crate) @keyword

(primitive_type) @type.builtin
(type_identifier) @type

(lifetime) @label

(function_item name: (identifier) @function)
(call_expression function: (identifier) @function)
(call_expression function: (field_expression field: (field_identifier) @method))
(macro_invocation macro: (identifier) @macro)

(attribute_item) @attribute
(inner_attribute_item) @attribute

(parameter pattern: (identifier) @parameter)
(field_identifier) @property
(scoped_identifier path: (identifier) @namespace)

[
  "+" "-" "*" "/" "%"
  "=" "==" "!=" "<" ">" "<=" ">="
  "&&" "||" "!"
  "&" "|" "^" "<<" ">>"
  "+=" "-=" "*=" "/=" "%=" "&=" "|=" "^=" "<<=" ">>="
  "@" "?" ".." "..="
] @operator

["(" ")" "{" "}" "[" "]"] @punctuation.bracket
["," ";" ":" "::" "->" "=>" "." "#"] @punctuation.delimiter

; SCREAMING_SNAKE_CASE identifiers read as constants (kept last so it
; overrides the generic identifier captures above).
((identifier) @constant (#match? @constant "^[A-Z_][A-Z0-9_]+$"))
