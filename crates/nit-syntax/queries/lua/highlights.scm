; Lua highlights (editor-grade, full token taxonomy)
; Grammar: tree-sitter-lua 0.5.0
; Node names / anonymous tokens verified against src/node-types.json and
; the upstream queries/highlights.scm of that exact version.

; ---------------------------------------------------------------------------
; Comments
; ---------------------------------------------------------------------------
(comment) @comment

; ---------------------------------------------------------------------------
; Literals
; ---------------------------------------------------------------------------
(string) @string
(escape_sequence) @escape

(number) @number

(true) @boolean
(false) @boolean

(nil) @constant.builtin
(vararg_expression) @constant

; UPPER_SNAKE identifiers read as constants.
((identifier) @constant
  (#match? @constant "^[A-Z][A-Z_0-9]*$"))

; ---------------------------------------------------------------------------
; Variables
; ---------------------------------------------------------------------------
(identifier) @variable

((identifier) @variable.builtin
  (#eq? @variable.builtin "self"))

; ---------------------------------------------------------------------------
; Keywords
; ---------------------------------------------------------------------------
; Control-flow keywords.
(if_statement
  [
    "if"
    "then"
    "elseif"
    "else"
    "end"
  ] @keyword.control)

(elseif_statement
  [
    "elseif"
    "then"
    "end"
  ] @keyword.control)

(else_statement
  [
    "else"
    "end"
  ] @keyword.control)

(while_statement
  [
    "while"
    "do"
    "end"
  ] @keyword.control)

(repeat_statement
  [
    "repeat"
    "until"
  ] @keyword.control)

(for_statement
  [
    "for"
    "do"
    "end"
  ] @keyword.control)

(do_statement
  [
    "do"
    "end"
  ] @keyword.control)

(return_statement
  "return" @keyword.control)

(break_statement) @keyword.control

(goto_statement
  "goto" @keyword.control)

; Declaration / binding keywords.
(function_declaration
  [
    "function"
    "end"
  ] @keyword)

(function_definition
  [
    "function"
    "end"
  ] @keyword)

[
  "local"
  "global"
  "in"
] @keyword

; Logical keyword-operators.
[
  "and"
  "or"
  "not"
] @keyword.operator

; ---------------------------------------------------------------------------
; Labels / attributes
; ---------------------------------------------------------------------------
(label_statement) @label

(variable_list
  (attribute
    "<" @punctuation.bracket
    (identifier) @attribute
    ">" @punctuation.bracket))

; ---------------------------------------------------------------------------
; Tables / fields / properties
; ---------------------------------------------------------------------------
(field
  name: (identifier) @property)

(dot_index_expression
  field: (identifier) @property)

(table_constructor
  [
    "{"
    "}"
  ] @constructor)

; ---------------------------------------------------------------------------
; Functions / methods
; ---------------------------------------------------------------------------
(parameters
  (identifier) @parameter)

(function_declaration
  name: [
    (identifier) @function
    (dot_index_expression
      field: (identifier) @function)
  ])

(function_declaration
  name: (method_index_expression
    method: (identifier) @method))

(assignment_statement
  (variable_list
    .
    name: [
      (identifier) @function
      (dot_index_expression
        field: (identifier) @function)
    ])
  (expression_list
    .
    value: (function_definition)))

(table_constructor
  (field
    name: (identifier) @function
    value: (function_definition)))

(function_call
  name: [
    (identifier) @function
    (dot_index_expression
      field: (identifier) @function)
    (method_index_expression
      method: (identifier) @method)
  ])

; Lua 5.1 standard-library builtins (override the generic call capture).
(function_call
  (identifier) @function
  (#any-of? @function
    "assert" "collectgarbage" "dofile" "error" "getfenv" "getmetatable"
    "ipairs" "load" "loadfile" "loadstring" "module" "next" "pairs" "pcall"
    "print" "rawequal" "rawget" "rawset" "require" "select" "setfenv"
    "setmetatable" "tonumber" "tostring" "type" "unpack" "xpcall"))

; ---------------------------------------------------------------------------
; Operators
; ---------------------------------------------------------------------------
[
  "+"
  "-"
  "*"
  "/"
  "//"
  "%"
  "^"
  "=="
  "~="
  "<="
  ">="
  "<"
  ">"
  "="
  "&"
  "|"
  "~"
  "<<"
  ">>"
  ".."
] @operator

; The `#` length operator only reads as an operator in a unary position;
; capture it there so the table-field `[` index / shebang `#` do not clash.
(unary_expression
  "#" @operator)

; ---------------------------------------------------------------------------
; Punctuation
; ---------------------------------------------------------------------------
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
  ":"
  "::"
  ","
  "."
] @punctuation.delimiter
