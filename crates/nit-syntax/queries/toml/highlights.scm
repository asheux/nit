; A dotted name uses namespace colors only in a table header.
(bare_key) @property
(quoted_key) @property
(table . [(bare_key) (quoted_key) (dotted_key)] @namespace)
(table_array_element . [(bare_key) (quoted_key) (dotted_key)] @namespace)
(table (dotted_key [(bare_key) (quoted_key)] @namespace))
(table_array_element (dotted_key [(bare_key) (quoted_key)] @namespace))

(comment) @comment
(string) @string
(escape_sequence) @escape
(boolean) @boolean
[(integer) (float)] @number
[(offset_date_time) (local_date_time) (local_date) (local_time)] @string.special

"=" @operator
["." ","] @punctuation.delimiter
["[" "]" "[[" "]]" "{" "}"] @punctuation.bracket
