; Dotenv highlights, riding on the bash grammar.
;
; `.env` files are shell-flavoured `KEY=value` assignments. The bash
; grammar parses each line as a `variable_assignment`, so we lean on
; that node to mark the left-hand side as a constant (uppercase env
; var) and the right-hand side as a string. Comments, quoted literals,
; and `${...}` expansions fall through to the standard bash captures
; with a couple of dotenv-specific overrides for booleans / numerics.

(comment) @comment

(variable_assignment
  name: (variable_name) @constant)

(variable_assignment
  value: (word) @string)

(variable_assignment
  value: (string) @string)

(variable_assignment
  value: (raw_string) @string)

(expansion
  (variable_name) @variable)

(simple_expansion
  (variable_name) @variable)

(string_content) @string

((word) @boolean
 (#match? @boolean "^(true|false|TRUE|FALSE|yes|no|on|off)$"))

((word) @number
 (#match? @number "^-?[0-9]+(\\.[0-9]+)?$"))
