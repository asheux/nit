; JSON highlights (editor-grade)
(string) @string
(escape_sequence) @escape
(number) @number

[
  (true)
  (false)
] @boolean
(null) @constant.builtin

; Object keys read as properties. Placed after the generic string rule so a
; key's string node is recolored from @string to @property.
(pair key: (string) @property)

["{" "}" "[" "]"] @punctuation.bracket
["," ":"] @punctuation.delimiter
