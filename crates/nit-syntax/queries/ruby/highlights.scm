; Ruby highlights (minimal, editor-grade)
(comment) @comment

(string) @string
(simple_symbol) @symbol
(heredoc_beginning) @string
(heredoc_body) @string

(integer) @number
(float) @number

(true) @boolean
(false) @boolean
(nil) @constant.builtin

[
  "def"
  "end"
  "class"
  "module"
  "if"
  "elsif"
  "else"
  "unless"
  "case"
  "when"
  "then"
  "do"
  "while"
  "until"
  "for"
  "in"
  "return"
  "yield"
  "begin"
  "rescue"
  "ensure"
  "retry"
  "redo"
  "next"
  "break"
  "and"
  "or"
  "not"
] @keyword

(method name: (identifier) @function)
(singleton_method name: (identifier) @function)
(call method: (identifier) @function.call)

(constant) @type
(instance_variable) @variable.member
(class_variable) @variable.member
(global_variable) @variable.builtin
