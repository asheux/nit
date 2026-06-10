; Go highlights (editor-grade, full token taxonomy)
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
  "map"
  "chan"
] @keyword

[
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
  "goto"
] @keyword.control

(type_identifier) @type

(function_declaration name: (identifier) @function)
(method_declaration name: (field_identifier) @function)
(call_expression function: (identifier) @function)
(call_expression function: (selector_expression field: (field_identifier) @method))

(parameter_declaration name: (identifier) @parameter)

[
  "+" "-" "*" "/" "%" "&" "|" "^" "<<" ">>" "&^"
  "&&" "||" "!" "==" "!=" "<" "<=" ">" ">=" "=" ":="
  "+=" "-=" "*=" "/=" "%=" "&=" "|=" "^=" "<<=" ">>=" "&^="
  "<-" "++" "--" "..."
] @operator

["(" ")" "{" "}" "[" "]"] @punctuation.bracket
["," ";" ":" "."] @punctuation.delimiter
