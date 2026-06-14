; Wolfram Language syntax highlights.
;
; The vendored bostick/tree-sitter-wolfram grammar is a generic
; operator-precedence parser: literals (integer / real / string / symbol),
; comments, and a large table of operator / bracket tokens wrapped in
; binary / infix / prefix / postfix / call / group nodes. There are no semantic
; nodes, so symbol roles are inferred from the name (Wolfram builtins are
; Capitalized). Override patterns run general -> specific; later matches win,
; the same convention queries/rust/highlights.scm relies on.

; --- Comments & literals ---------------------------------------------------
(comment) @comment

(string) @string
(integer) @number
(real) @number

; --- Symbols ---------------------------------------------------------------
; Default to a plain variable; Capitalized names are Wolfram builtins surfaced
; as functions, then control-flow, booleans and constants are carved back out.
(symbol) @variable

((symbol) @function
  (#match? @function "^[A-Z]"))

((symbol) @keyword.control
  (#match? @keyword.control "^(If|While|For|Do|Switch|Which|Module|Block|With|Function|Return|Throw|Catch|Break|Continue|Goto|Abort)$"))

((symbol) @boolean
  (#match? @boolean "^(True|False)$"))

((symbol) @constant.builtin
  (#match? @constant.builtin "^(Pi|E|I|Infinity|ComplexInfinity|Indeterminate|Null|Degree|GoldenRatio|EulerGamma|Catalan|Glaisher|Khinchin|All|None|Automatic)$"))

; --- Operators -------------------------------------------------------------
[
  "!" "!!" "!=" "&" "&&" "'"
  "*" "**" "*=" "+" "++" "+=" "-" "--" "-=" "->"
  "." ".." "..." "/" "/*" "/." "//" "//." "//=" "//@" "/;" "/=" "/@"
  ":=" ":>" "<" "<->" "<=" "<>" "=" "=!=" "==" "===" ">" ">=" "?"
  "@" "@*" "@@" "@@@" "^" "^:=" "^=" "|" "|->" "||" "~~"
] @operator

; --- Punctuation -----------------------------------------------------------
[
  "(" ")" "[" "]" "{" "}" "<|" "|>"
] @punctuation.bracket

[
  "," ";"
] @punctuation.delimiter
