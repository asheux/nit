; Lua highlights (minimal, editor-grade)
(comment) @comment

(string) @string

(number) @number

(true) @boolean
(false) @boolean
(nil) @constant.builtin

[
  "function"
  "end"
  "local"
  "if"
  "then"
  "elseif"
  "else"
  "while"
  "do"
  "for"
  "repeat"
  "until"
  "return"
  "and"
  "or"
  "not"
  "in"
  "goto"
] @keyword

(function_declaration name: (identifier) @function)
(function_call name: (identifier) @function.call)
