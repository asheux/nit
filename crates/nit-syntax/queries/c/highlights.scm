; C highlights (minimal, editor-grade)
(comment) @comment

(string_literal) @string
(system_lib_string) @string
(char_literal) @character

(number_literal) @number

(true) @boolean
(false) @boolean
(null) @constant.builtin

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
  "static"
  "const"
  "extern"
  "register"
  "auto"
  "volatile"
  "inline"
] @keyword

(primitive_type) @type.builtin
(type_identifier) @type
(sized_type_specifier) @type.builtin

(function_declarator declarator: (identifier) @function)
(call_expression function: (identifier) @function.call)
(call_expression function: (field_expression field: (field_identifier) @function.call))

(field_identifier) @variable.member
(preproc_directive) @keyword.directive
