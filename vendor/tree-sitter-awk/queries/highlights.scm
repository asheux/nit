; tree-sitter-awk v0.7.2 highlights, vendored from Beaglefoot/tree-sitter-awk
; (queries/highlights.scm at commit 34bbdc7cce8e803096f47b625979e34c1be38127).
;
; https://tree-sitter.github.io/tree-sitter/using-parsers#pattern-matching-with-queries
;
; nit edits against upstream (re-apply when re-vendoring):
;   * `@regexp` is `@string.special`, the capture name nit's table knows.
;   * Brackets and delimiters are `@punctuation.bracket` /
;     `@punctuation.delimiter` instead of `@operator`, like every other nit
;     query.
;   * Function names: upstream's `name: (_ (identifier) @function)` only
;     matches namespace-qualified names (a bare `identifier` has no child), so
;     `function foo()` and `foo()` stayed `@variable`. Explicit
;     `name: (identifier)` patterns are added, and the builtin call names
;     (`length`, `split`, `sub`, ...) are `@function.builtin`.
;   * Order matters: nit's highlighter (tree-sitter-highlight 0.25) applies the
;     LAST matching pattern to a node, so patterns run general -> specific and
;     the `(identifier) @variable` catch-all comes before the function-name
;     patterns rather than after them as upstream has it.

; --- Comments & literals ---------------------------------------------------
(comment) @comment
(string) @string
(number) @number
(regex) @string.special

; --- Variables -------------------------------------------------------------
[
  (identifier)
  (field_ref)
] @variable
(field_ref (_) @variable)

; --- Namespaces ------------------------------------------------------------
(ns_qualified_name (namespace) @namespace)
(ns_qualified_name "::" @operator)

; --- Functions (after the identifier catch-all so they win) ---------------
(func_def name: (identifier) @function)
(func_def name: (ns_qualified_name (identifier) @function))
(func_call name: (identifier) @function)
(func_call name: (ns_qualified_name (identifier) @function))

[
  "and"
  "asort"
  "asorti"
  "bindtextdomain"
  "compl"
  "cos"
  "dcgettext"
  "dcngettext"
  "exp"
  "gensub"
  "gsub"
  "index"
  "int"
  "isarray"
  "length"
  "log"
  "lshift"
  "match"
  "mktime"
  "or"
  "patsplit"
  "rand"
  "rshift"
  "sin"
  "split"
  "sprintf"
  "sqrt"
  "srand"
  "strftime"
  "strtonum"
  "sub"
  "substr"
  "systime"
  "tolower"
  "toupper"
  "typeof"
  "xor"
] @function.builtin

; --- Keywords --------------------------------------------------------------
[
  "function"
  "func"
  "print"
  "printf"
  "if"
  "else"
  "do"
  "while"
  "for"
  "in"
  "delete"
  "return"
  "exit"
  "switch"
  "case"
  "default"
  (break_statement)
  (continue_statement)
  (next_statement)
  (nextfile_statement)
  (getline_input)
  (getline_file)
] @keyword

[
  "@include"
  "@load"
  "@namespace"
  (pattern)
] @namespace

; --- Operators -------------------------------------------------------------
(binary_exp [
  "^"
  "**"
  "*"
  "/"
  "%"
  "+"
  "-"
  "<"
  ">"
  "<="
  ">="
  "=="
  "!="
  "~"
  "!~"
  "in"
  "&&"
  "||"
] @operator)

(unary_exp [
  "!"
  "+"
  "-"
] @operator)

(assignment_exp [
  "="
  "+="
  "-="
  "*="
  "/="
  "%="
  "^="
] @operator)

(ternary_exp [
  "?"
  ":"
] @operator)

(update_exp [
  "++"
  "--"
] @operator)

(redirected_io_statement [
  ">"
  ">>"
] @operator)

(piped_io_statement [
  "|"
  "|&"
] @operator)

; --- Punctuation -----------------------------------------------------------
[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

[
  ";"
  ","
] @punctuation.delimiter
