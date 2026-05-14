; C++ highlights (minimal, editor-grade)
(comment) @comment

(string_literal) @string
(raw_string_literal) @string
(char_literal) @character

(number_literal) @number

[
  "if"
  "else"
  "for"
  "while"
  "do"
  "switch"
  "case"
  "default"
  "break"
  "continue"
  "return"
  "goto"
  "sizeof"
  "typedef"
  "struct"
  "union"
  "enum"
  "class"
  "namespace"
  "template"
  "typename"
  "using"
  "public"
  "private"
  "protected"
  "static"
  "const"
  "constexpr"
  "extern"
  "explicit"
  "inline"
  "operator"
  "new"
  "delete"
  "try"
  "catch"
  "throw"
] @keyword

(primitive_type) @type.builtin
(type_identifier) @type
(sized_type_specifier) @type.builtin

(function_declarator declarator: (identifier) @function)
(call_expression function: (identifier) @function.call)
