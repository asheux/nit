; Zig highlights (tree-sitter-zig 1.x)
(comment) @comment

(string) @string
(multiline_string) @string

(integer) @number

(boolean) @boolean

[
  "const"
  "var"
  "fn"
  "pub"
  "if"
  "else"
  "while"
  "for"
  "switch"
  "return"
  "break"
  "continue"
  "defer"
  "try"
  "catch"
  "struct"
  "enum"
  "union"
  "test"
  "comptime"
  "inline"
  "async"
  "await"
  "and"
  "or"
] @keyword
