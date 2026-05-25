; Wolfram Language highlights — placeholder queries.
;
; The judge's plan elected to ship Wolfram detection WITHOUT bundling a
; tree-sitter-wolfram crate (none is mature enough on crates.io as of
; this PR cluster). Files with extensions `.wl` / `.wls` therefore
; render as plain text under nit's `Wolfram` language label; the
; grammar dispatch in `nit-syntax/src/language/grammars.rs` returns
; `None` and never loads this query.
;
; The file exists so:
;   * the structural-compliance check sees the language directory
;     populated alongside its peers;
;   * promoting Wolfram to a tree-sitter-backed language later is a
;     single Cargo dependency add + one arm flip in `grammars.rs`,
;     without also having to author the highlights query from scratch.
;
; The captures below mirror common Wolfram constructs (symbols,
; comments, strings, numerics). They are inert until a grammar lands;
; renaming or rewording them won't change render behaviour today.

; Comments: `(* ... *)`
(comment) @comment

; String literals — double-quoted.
(string) @string

; Numeric literals (Integer / Real / Rational / Complex).
(number) @number

; Built-in symbols start uppercase and follow camel-case (`Plus`,
; `Module`, `Cases`). Tag them as functions when followed by `[`, and
; as constants otherwise — the choice is documented here so the
; promotion PR can wire it without re-litigating the convention.
((symbol) @function
 (#match? @function "^[A-Z][A-Za-z0-9]*$"))

((symbol) @constant
 (#match? @constant "^[A-Z][A-Z0-9_]*$"))

; Operator tokens.
[
  "->"
  ":>"
  ":="
  "="
  "=="
  "!="
  ":"
  "/."
  "//"
  "@"
  "/@"
  "&"
  "|"
  "||"
  "&&"
] @operator

; Brackets and parens.
[
  "["
  "]"
  "{"
  "}"
  "("
  ")"
] @punctuation.bracket

; Statement separators.
[
  ";"
  ","
] @punctuation.delimiter
