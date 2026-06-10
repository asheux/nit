; Zig highlights (editor-grade, full token taxonomy; tree-sitter-zig 1.1.x)

; ── Comments ────────────────────────────────────────────────────────────────
(comment) @comment
; Doc comments (//! and ///) override the generic comment capture.
((comment) @comment.documentation
  (#match? @comment.documentation "^//[!/]"))

; ── Literals ────────────────────────────────────────────────────────────────
(string) @string
(multiline_string) @string
(character) @character
(escape_sequence) @escape

(integer) @number
(float) @number

(boolean) @boolean

[
  "null"
  "unreachable"
  "undefined"
] @constant.builtin

; ── Types ───────────────────────────────────────────────────────────────────
[
  (builtin_type)
  "anyframe"
] @type.builtin

; PascalCase identifiers read as types.
((identifier) @type
  (#match? @type "^[A-Z][a-zA-Z0-9_]*$"))

; Identifier bound to a struct/enum/union/opaque declaration is a type.
(variable_declaration
  (identifier) @type
  "="
  [
    (struct_declaration)
    (enum_declaration)
    (union_declaration)
    (opaque_declaration)
  ])

; ── Constants ───────────────────────────────────────────────────────────────
((identifier) @constant
  (#match? @constant "^[A-Z][A-Z_0-9]+$"))

; ── Functions / builtins ────────────────────────────────────────────────────
(function_declaration
  name: (identifier) @function)

(call_expression
  function: (identifier) @function)

(call_expression
  function: (field_expression
    member: (identifier) @method))

; @import / @sizeOf / @cImport / ... builtins.
(builtin_identifier) @macro

; ── Parameters / fields / namespaces ────────────────────────────────────────
(parameter
  name: (identifier) @parameter)

(field_expression
  member: (identifier) @property)

(field_initializer
  (identifier) @property)

(container_field
  name: (identifier) @property)

; ── Labels ──────────────────────────────────────────────────────────────────
(block_label (identifier) @label)
(break_label (identifier) @label)

; ── Builtin variables ───────────────────────────────────────────────────────
[
  "c"
  "..."
] @variable.builtin

((identifier) @variable.builtin
  (#eq? @variable.builtin "_"))

; ── Keywords: control flow ──────────────────────────────────────────────────
[
  "if"
  "else"
  "switch"
  "for"
  "while"
  "break"
  "continue"
  "return"
  "try"
  "catch"
  "defer"
  "errdefer"
  "async"
  "await"
  "suspend"
  "nosuspend"
  "resume"
] @keyword.control

; ── Keywords: logical operators ─────────────────────────────────────────────
[
  "and"
  "or"
  "orelse"
] @keyword.operator

; ── Keywords: declarations / modifiers / misc ───────────────────────────────
[
  "const"
  "var"
  "fn"
  "pub"
  "struct"
  "enum"
  "union"
  "opaque"
  "error"
  "test"
  "comptime"
  "inline"
  "noinline"
  "extern"
  "export"
  "packed"
  "threadlocal"
  "volatile"
  "allowzero"
  "noalias"
  "addrspace"
  "align"
  "callconv"
  "linksection"
  "usingnamespace"
  "asm"
] @keyword

; ── Operators ───────────────────────────────────────────────────────────────
[
  "="
  "*="
  "*%="
  "*|="
  "/="
  "%="
  "+="
  "+%="
  "+|="
  "-="
  "-%="
  "-|="
  "<<="
  "<<|="
  ">>="
  "&="
  "^="
  "|="
  "!"
  "~"
  "-"
  "-%"
  "&"
  "=="
  "!="
  ">"
  ">="
  "<="
  "<"
  "^"
  "|"
  "<<"
  ">>"
  "<<|"
  "+"
  "++"
  "+%"
  "+|"
  "-|"
  "*"
  "/"
  "%"
  "**"
  "*%"
  "*|"
  "||"
  ".*"
  ".?"
  "?"
  ".."
] @operator

; ── Punctuation ─────────────────────────────────────────────────────────────
[
  "["
  "]"
  "("
  ")"
  "{"
  "}"
] @punctuation.bracket

(payload "|" @punctuation.bracket)

[
  ";"
  "."
  ","
  ":"
  "=>"
  "->"
] @punctuation.delimiter
