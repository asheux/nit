; Python highlights (minimal, editor-grade)
(comment) @comment
(string) @string

(integer) @number
(float) @number

["True" "False"] @boolean
["None"] @constant

[
  "def"
  "class"
  "return"
  "if"
  "elif"
  "else"
  "for"
  "while"
  "try"
  "except"
  "finally"
  "with"
  "as"
  "import"
  "from"
  "pass"
  "break"
  "continue"
  "raise"
  "yield"
  "lambda"
  "global"
  "nonlocal"
  "assert"
  "async"
  "await"
] @keyword

(function_definition name: (identifier) @function)
(class_definition name: (identifier) @type)

(call function: (identifier) @function)
