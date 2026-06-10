; Elixir highlights (editor-grade, full token taxonomy)
; Node names + anonymous tokens validated against tree-sitter-elixir 0.3.5
; (src/node-types.json + upstream queries/highlights.scm). Most "keywords"
; in Elixir (def, defmodule, if, ...) are macros the grammar parses as the
; target identifier of a `call`; we promote those identifiers to @keyword /
; @keyword.control via call-target patterns below.
; tree-sitter precedence: later patterns override earlier ones for the same
; node, so generic captures come first and specific overrides come last.

; ── Comments ────────────────────────────────────────────────────────────
(comment) @comment

; ── Literals ────────────────────────────────────────────────────────────
(string) @string
(charlist) @string

(escape_sequence) @escape

(char) @character

(integer) @number
(float) @number

(boolean) @boolean
(nil) @constant.builtin

; Atoms / keyword-list keys render as symbolic constants.
; (@symbol is the original capture; @string.special is the nit-recognised name.)
(atom) @symbol
[
  (atom)
  (quoted_atom)
  (keyword)
  (quoted_keyword)
] @string.special

; Sigils (~s, ~r, ~w, ...): mark name + the quoted delimiters.
(sigil
  (sigil_name) @macro
  quoted_start: _ @string.special
  quoted_end: _ @string.special) @string.special

; ── Identifiers ─────────────────────────────────────────────────────────
; Generic base; calls / keywords / attributes below override where relevant.
(identifier) @variable

; Module names (Foo.Bar) are aliases → namespaces.
(alias) @namespace

; ── Interpolation ───────────────────────────────────────────────────────
(interpolation "#{" @punctuation.delimiter "}" @punctuation.delimiter)

; ── Operators (named-field based, mirrors upstream) ─────────────────────
(operator_identifier) @operator

(unary_operator
  operator: _ @operator)

(binary_operator
  operator: _ @operator)

(dot
  operator: _ @operator)

(stab_clause
  operator: _ @operator)

; ── Operators (explicit anonymous token list) ───────────────────────────
[
  "!" "!=" "!=="
  "&" "&&" "&&&"
  "*" "**"
  "+" "++" "+++"
  "-" "--" "---"
  "/" "//"
  "<" "<-" "<<<" "<<~" "<=" "<>" "<|>" "<~" "<~>"
  "=" "==" "===" "=~"
  ">" ">=" ">>>"
  "^" "^^^"
  "|" "|>" "||" "|||"
  "~" "~>" "~>>" "~~~"
  "@"
] @operator

; ── Punctuation ─────────────────────────────────────────────────────────
[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
  "<<"
  ">>"
] @punctuation.bracket

[
  ","
  ";"
  "."
  ":"
  "::"
  "->"
  "=>"
] @punctuation.delimiter

["%"] @punctuation

; ── Calls ───────────────────────────────────────────────────────────────
; * local function call
(call
  target: (identifier) @function)

; * remote function call (Foo.bar(...))
(call
  target: (dot
    right: (identifier) @function))

; * field access without parentheses/block (foo.bar)
(call
  target: (dot
    right: (identifier) @property)
  .)

; * remote call without parentheses/block overrides the property case
(call
  target: (dot
    left: [
      (alias)
      (atom)
    ]
    right: (identifier) @function)
  .)

; * pipe into identifier → function call
(binary_operator
  operator: "|>"
  right: (identifier) @function)

(binary_operator
  operator: "|>"
  right: (call
    target: (dot
      right: (identifier) @function)))

; ── Definition / special-form keywords (call targets) ───────────────────
; * definition keywords
(call
  target: (identifier) @keyword
  (#any-of? @keyword
    "def" "defp" "defmodule" "defprotocol" "defimpl" "defstruct"
    "defexception" "defdelegate" "defguard" "defguardp"
    "defmacro" "defmacrop" "defn" "defnp" "defoverridable"))

; * the function name being defined is a function, not a keyword
(call
  target: (identifier) @keyword
  (arguments
    [
      (identifier) @function
      (binary_operator
        left: (identifier) @function
        operator: "when")
    ])
  (#any-of? @keyword
    "def" "defp" "defdelegate" "defguard" "defguardp"
    "defmacro" "defmacrop" "defn" "defnp"))

; * kernel / special-form control keywords
(call
  target: (identifier) @keyword.control
  (#any-of? @keyword.control
    "case" "cond" "for" "if" "unless" "with" "receive" "try"))

; * kernel / special-form non-control keywords
(call
  target: (identifier) @keyword
  (#any-of? @keyword
    "alias" "import" "require" "use" "quote" "unquote"
    "unquote_splicing" "raise" "reraise" "throw" "super"))

; ── Module attributes (@attr) and doc strings ───────────────────────────
(unary_operator
  operator: "@" @attribute
  operand: [
    (identifier) @attribute
    (call
      target: (identifier) @attribute)
    (boolean) @attribute
    (nil) @attribute
  ])

; * @moduledoc / @doc / @typedoc string content → documentation comment
(unary_operator
  operator: "@" @comment.documentation
  operand: (call
    target: (identifier) @comment.documentation
    (arguments
      [
        (string) @comment.documentation
        (charlist) @comment.documentation
        (sigil
          quoted_start: _ @comment.documentation
          quoted_end: _ @comment.documentation) @comment.documentation
        (boolean) @comment.documentation
      ]))
  (#any-of? @comment.documentation "moduledoc" "typedoc" "doc"))

; ── Reserved keyword tokens ─────────────────────────────────────────────
; Control-flow / clause keywords.
[
  "after"
  "catch"
  "else"
  "rescue"
] @keyword.control

; Operator-like reserved words.
[
  "and"
  "in"
  "not"
  "not in"
  "or"
  "when"
] @keyword.operator

; Remaining reserved words / block delimiters.
[
  "do"
  "end"
  "fn"
] @keyword

; ── Builtin constants (override identifier @variable) ───────────────────
((identifier) @constant.builtin
  (#any-of? @constant.builtin
    "__MODULE__" "__DIR__" "__ENV__" "__CALLER__" "__STACKTRACE__"))

; Unused / underscore-prefixed bindings dim like comments.
((identifier) @comment
  (#match? @comment "^_"))
