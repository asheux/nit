; Go highlights (minimal, editor-grade)
(comment) @comment

(interpreted_string_literal) @string
(raw_string_literal) @string
(rune_literal) @character

(int_literal) @number
(float_literal) @number
(imaginary_literal) @number

(true) @boolean
(false) @boolean
(nil) @constant.builtin

[
  "func"
  "var"
  "const"
  "type"
  "struct"
  "interface"
  "import"
  "package"
  "return"
  "if"
  "else"
  "for"
  "range"
  "switch"
  "case"
  "default"
  "break"
  "continue"
  "fallthrough"
  "go"
  "defer"
  "select"
  "chan"
  "map"
  "goto"
] @keyword

(function_declaration name: (identifier) @function)
(method_declaration name: (field_identifier) @function)
(call_expression function: (identifier) @function.call)
(call_expression function: (selector_expression field: (field_identifier) @function.call))

(type_identifier) @type
(field_identifier) @variable.member
