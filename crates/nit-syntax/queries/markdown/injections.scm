; Markdown injections (fenced code blocks)
; tree-sitter-md 0.3 nests the language identifier inside info_string
; as a `language` child node; capturing info_string directly would grab
; surrounding whitespace and miss the match.
(
  fenced_code_block
  (info_string
    (language) @injection.language)
  (code_fence_content) @injection.content
)
