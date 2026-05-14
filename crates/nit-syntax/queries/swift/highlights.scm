; Swift highlights (minimal, editor-grade)
(comment) @comment
(multiline_comment) @comment

(line_string_literal) @string
(multi_line_string_literal) @string
(raw_string_literal) @string

(integer_literal) @number
(real_literal) @number
(hex_literal) @number
(oct_literal) @number
(bin_literal) @number

(boolean_literal) @boolean

[
  "let"
  "var"
  "func"
  "class"
  "struct"
  "enum"
  "protocol"
  "extension"
  "if"
  "for"
  "while"
  "do"
  "switch"
  "case"
  "break"
  "continue"
  "return"
  "guard"
  "try"
  "import"
  "public"
  "private"
  "internal"
  "fileprivate"
  "open"
  "static"
  "final"
  "lazy"
  "weak"
  "in"
  "as"
  "is"
  "self"
  "super"
] @keyword

(simple_identifier) @variable
(type_identifier) @type
