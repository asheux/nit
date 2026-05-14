; Elixir highlights (minimal, editor-grade)
; Most "keywords" in Elixir (def, defmodule, if, ...) are macros that the
; grammar parses as identifier calls; we promote those identifiers to
; @keyword via a do_block-aware pattern below.
(comment) @comment

(string) @string
(charlist) @string

(integer) @number
(float) @number

(boolean) @boolean
(nil) @constant.builtin
(atom) @symbol

; Macro-style keywords appear as the target identifier of a call.
((identifier) @keyword
 (#match? @keyword "^(def|defp|defmodule|defmacro|defmacrop|defprotocol|defimpl|defstruct|defguard|defguardp|defdelegate|if|unless|case|cond|for|when|with|try|catch|rescue|after|import|alias|require|use|raise|throw)$"))

; "do"/"end" / "fn" are anonymous tokens in tree-sitter-elixir.
[
  "do"
  "end"
  "fn"
  "in"
] @keyword
