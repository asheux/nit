; YAML highlights (editor-grade, full token taxonomy)
; Node names + anonymous tokens verified against tree-sitter-yaml 0.7.2
; (src/node-types.json and upstream queries/highlights.scm).

; --- Comments ---
(comment) @comment

; --- Strings (generic; property-key overrides come later) ---
(string_scalar) @string
(plain_scalar) @string
[
  (double_quote_scalar)
  (single_quote_scalar)
  (block_scalar)
] @string

; --- Escape sequences inside quoted scalars ---
(escape_sequence) @escape

; --- Numbers / timestamps ---
(integer_scalar) @number
(float_scalar) @number
(timestamp_scalar) @number

; --- Booleans / language constants ---
(boolean_scalar) @boolean
(null_scalar) @constant.builtin

; --- Tags / type system ---
(tag) @type
(tag_handle) @type.builtin
(tag_prefix) @type

; --- Anchors / aliases ---
[
  (anchor_name)
  (alias_name)
] @label

; --- Directives (annotations) ---
[
  (yaml_directive)
  (tag_directive)
  (reserved_directive)
] @attribute
(directive_name) @keyword
(directive_parameter) @parameter
(yaml_version) @number

; --- Mapping keys -> property (specific; overrides the generic string rules above) ---
(block_mapping_pair
  key: (flow_node
    [
      (double_quote_scalar)
      (single_quote_scalar)
    ] @property))

(block_mapping_pair
  key: (flow_node
    (plain_scalar
      (string_scalar) @property)))

(flow_mapping
  (_
    key: (flow_node
      [
        (double_quote_scalar)
        (single_quote_scalar)
      ] @property)))

(flow_mapping
  (_
    key: (flow_node
      (plain_scalar
        (string_scalar) @property))))

; --- Operators (anchor/alias sigils) ---
[
  "*"
  "&"
] @operator

; --- Brackets ---
[
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

; --- Delimiters / document markers ---
[
  ","
  "-"
  ":"
  ">"
  "?"
  "|"
  "---"
  "..."
] @punctuation.delimiter
