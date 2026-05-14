; PHP highlights (minimal, editor-grade)
(comment) @comment

(string) @string
(encapsed_string) @string
(heredoc) @string

(integer) @number
(float) @number

(boolean) @boolean
(null) @constant.builtin

[
  "function"
  "class"
  "interface"
  "trait"
  "extends"
  "implements"
  "public"
  "private"
  "protected"
  "static"
  "final"
  "abstract"
  "if"
  "elseif"
  "else"
  "for"
  "foreach"
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
  "new"
  "echo"
  "print"
  "use"
  "namespace"
  "as"
  "instanceof"
  "global"
  "const"
] @keyword

(function_definition name: (name) @function)
(method_declaration name: (name) @function)
(function_call_expression function: (name) @function.call)

(variable_name) @variable
