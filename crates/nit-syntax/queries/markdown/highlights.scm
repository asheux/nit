; Markdown highlights — BLOCK-LEVEL grammar only.
;
; tree-sitter-md splits the parser in two: `LANGUAGE` (this one) parses the
; block structure (headings, lists, code blocks, block quotes, tables, link
; reference definitions) and `INLINE_LANGUAGE` parses inline spans (emphasis,
; strong, inline code spans, autolinks, inline links/images). Rich inline
; highlighting (emphasis / strong / code_span / inline link_text) therefore
; requires wiring the inline sub-parser as an INJECTION on the `(inline)` node —
; until that injection is live, none of those inline node types exist in this
; tree, so we only capture nodes the BLOCK grammar actually produces.
;
; Every named node and anonymous token below was confirmed against this
; grammar's src/node-types.json (tree-sitter-md 0.3.2). This grammar is markup,
; not a programming language: it exposes no keywords, types, functions,
; parameters, or arithmetic/comparison/logical operators, so those capture
; groups have no applicable nodes here.

; --- Headings ------------------------------------------------------------
(atx_heading) @heading
(setext_heading) @heading

; Heading markers (the `#`..`######` prefixes and the `===`/`---` underlines).
[
  (atx_h1_marker)
  (atx_h2_marker)
  (atx_h3_marker)
  (atx_h4_marker)
  (atx_h5_marker)
  (atx_h6_marker)
  (setext_h1_underline)
  (setext_h2_underline)
] @punctuation.delimiter

; --- Code blocks ---------------------------------------------------------
(fenced_code_block) @string
(indented_code_block) @string

; The ``` / ~~~ fences override the broad code-block string capture above.
(fenced_code_block_delimiter) @punctuation.delimiter

; The language tag after an opening fence (```rust -> "rust").
(info_string) @label
(language) @label

; --- Block quotes & thematic breaks --------------------------------------
(thematic_break) @comment
(block_quote) @comment

; The `>` quote marker, overriding the broad block_quote @comment above.
(block_quote_marker) @punctuation.delimiter

; --- Lists ---------------------------------------------------------------
[
  (list_marker_plus)
  (list_marker_minus)
  (list_marker_star)
  (list_marker_dot)
  (list_marker_parenthesis)
] @punctuation.delimiter

; GFM task-list checkboxes ( [ ] / [x] ).
[
  (task_list_marker_checked)
  (task_list_marker_unchecked)
] @constant.builtin

; --- Link reference definitions ------------------------------------------
;   [label]: destination "title"
(link_label) @label
(link_destination) @link
(link_title) @string

; --- Tables (GFM pipe tables) --------------------------------------------
; Column alignment markers (`:` ends in the delimiter row).
[
  (pipe_table_align_left)
  (pipe_table_align_right)
] @punctuation.delimiter

; --- Character references & escapes ---------------------------------------
(entity_reference) @constant
(numeric_character_reference) @constant
(backslash_escape) @escape
