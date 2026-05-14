; Kotlin highlights (tree-sitter-kotlin-ng)
(line_comment) @comment
(block_comment) @comment

(string_literal) @string
(character_literal) @character

(number_literal) @number

[
  "fun"
  "val"
  "var"
  "class"
  "interface"
  "object"
  "enum"
  "data"
  "sealed"
  "if"
  "else"
  "for"
  "while"
  "do"
  "when"
  "return"
  "try"
  "catch"
  "finally"
  "throw"
  "import"
  "package"
  "public"
  "private"
  "protected"
  "internal"
  "abstract"
  "open"
  "final"
  "override"
  "lateinit"
  "by"
  "in"
  "out"
  "as"
  "is"
] @keyword
