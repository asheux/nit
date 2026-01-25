; JavaScript highlights (minimal, editor-grade)
(comment) @comment
(string) @string
(template_string) @string

(number) @number
["true" "false"] @boolean
["null" "undefined"] @constant

[
  "function"
  "return"
  "if"
  "else"
  "for"
  "while"
  "switch"
  "case"
  "break"
  "continue"
  "try"
  "catch"
  "throw"
  "class"
  "extends"
  "new"
  "import"
  "export"
  "from"
  "const"
  "let"
  "var"
  "async"
  "await"
  "yield"
] @keyword

(function_declaration name: (identifier) @function)
(method_definition name: (property_identifier) @method)
(call_expression function: (identifier) @function)
(member_expression property: (property_identifier) @property)
