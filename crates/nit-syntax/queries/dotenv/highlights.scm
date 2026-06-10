; Dotenv highlights, riding on the bash grammar (tree-sitter-bash 0.23).
;
; `.env` files are shell-flavoured `KEY=value` assignments. The bash
; grammar parses each line as a `variable_assignment`, so we lean on
; that node to mark the left-hand side as a constant (uppercase env
; var) and the right-hand side as a string. Comments, quoted literals,
; and `${...}` expansions fall through to the standard bash captures
; with a couple of dotenv-specific overrides for booleans / numerics.
;
; Precedence note: tree-sitter applies the LAST matching pattern for a
; given node, so the generic bash captures come first and the
; dotenv-specific overrides (constant LHS, boolean/number RHS) come
; last. Every named node and anonymous token below is confirmed present
; in tree-sitter-bash 0.23.3's grammar (src/node-types.json); the
; grammar has no escape-sequence / char / boolean / type / decorator
; nodes, so those capture groups are intentionally absent.

; --- Comments ---------------------------------------------------------
(comment) @comment

; --- Strings ----------------------------------------------------------
[
  (string)
  (raw_string)
  (ansi_c_string)
  (translated_string)
  (heredoc_body)
  (heredoc_start)
] @string

(string_content) @string
(regex) @string.special

; --- Numbers ----------------------------------------------------------
(number) @number
(file_descriptor) @number

; --- Keywords (non-control) ------------------------------------------
[
  "declare"
  "typeset"
  "export"
  "readonly"
  "local"
  "unset"
  "unsetenv"
  "function"
  "in"
] @keyword

; --- Keywords (control flow) -----------------------------------------
[
  "if"
  "then"
  "elif"
  "else"
  "fi"
  "case"
  "esac"
  "for"
  "while"
  "until"
  "do"
  "done"
  "select"
] @keyword.control

; --- Functions & commands --------------------------------------------
(command_name) @function
(function_definition name: (word) @function)

; --- Variables & properties ------------------------------------------
(variable_name) @property
(special_variable_name) @variable.builtin

(expansion (variable_name) @variable)
(simple_expansion (variable_name) @variable)

; --- Operators --------------------------------------------------------
(test_operator) @operator

[
  "="
  "=="
  "!="
  "=~"
  "<"
  ">"
  "<="
  ">="
  "+"
  "-"
  "*"
  "/"
  "%"
  "**"
  "++"
  "--"
  "+="
  "-="
  "*="
  "/="
  "%="
  "**="
  "&="
  "|="
  "^="
  "&&"
  "||"
  "!"
  "&"
  "|"
  "^"
  "~"
  "?"
  ".."
  "$"
  ">>"
  "<<"
  "<<<"
  ">&"
  "<&"
  ">|"
  "|&"
] @operator

; --- Brackets ---------------------------------------------------------
[
  "("
  ")"
  "(("
  "))"
  "{"
  "}"
  "["
  "]"
  "[["
  "]]"
  "$("
  "$(("
  "$["
  "${"
] @punctuation.bracket

; --- Delimiters -------------------------------------------------------
[
  ";"
  ";;"
  ","
] @punctuation.delimiter

; --- Dotenv-specific overrides (must remain last) ---------------------
; `KEY=value` left-hand side is the env-var constant.
(variable_assignment
  name: (variable_name) @constant)

; Bare right-hand-side values render as strings.
(variable_assignment
  value: (word) @string)

(variable_assignment
  value: (string) @string)

(variable_assignment
  value: (raw_string) @string)

; Common boolean-ish / numeric `.env` values, matched on the bare word.
((word) @boolean
 (#match? @boolean "^(true|false|TRUE|FALSE|yes|no|on|off)$"))

((word) @number
 (#match? @number "^-?[0-9]+(\\.[0-9]+)?$"))
