; Markdown highlights (block-level only — tree-sitter-md splits inline
; into a separate parser that we'd need to wire via injections for
; emphasis / code_span / link_* etc.). Until those are wired, keep the
; query to nodes that exist in the block grammar.
(atx_heading) @heading
(setext_heading) @heading

(fenced_code_block) @string
(indented_code_block) @string

(thematic_break) @comment
(block_quote) @comment
