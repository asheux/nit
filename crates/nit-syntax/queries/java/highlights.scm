; Java highlights (minimal, editor-grade)
(line_comment) @comment
(block_comment) @comment

(string_literal) @string
(character_literal) @character

(decimal_integer_literal) @number
(hex_integer_literal) @number
(octal_integer_literal) @number
(binary_integer_literal) @number
(decimal_floating_point_literal) @number
(hex_floating_point_literal) @number

(true) @boolean
(false) @boolean
(null_literal) @constant.builtin

[
  "class"
  "interface"
  "enum"
  "extends"
  "implements"
  "public"
  "private"
  "protected"
  "static"
  "final"
  "abstract"
  "synchronized"
  "volatile"
  "transient"
  "native"
  "strictfp"
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
  "try"
  "catch"
  "finally"
  "throw"
  "throws"
  "new"
  "instanceof"
  "import"
  "package"
] @keyword

(method_declaration name: (identifier) @function)
(method_invocation name: (identifier) @function.call)

(class_declaration name: (identifier) @type)
(interface_declaration name: (identifier) @type)
(type_identifier) @type
