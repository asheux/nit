; Swift highlights (editor-grade, full token taxonomy)
; Node names and anonymous tokens verified against tree-sitter-swift 0.7.2
; (src/node-types.json) and mirrored from its upstream queries/highlights.scm.
; Generic captures come first; specific overrides come last so they win
; (tree-sitter precedence: later patterns override earlier for the same node).

; ---------------------------------------------------------------------------
; Comments
; ---------------------------------------------------------------------------
(comment) @comment
(multiline_comment) @comment

((comment) @comment.documentation
  (#match? @comment.documentation "^///[^/]"))
((comment) @comment.documentation
  (#match? @comment.documentation "^///$"))
((multiline_comment) @comment.documentation
  (#match? @comment.documentation "^/[*][*][^*].*[*]/$"))

; ---------------------------------------------------------------------------
; Strings, interpolation, escapes, regex
; ---------------------------------------------------------------------------
(line_string_literal) @string
(multi_line_string_literal) @string
(raw_string_literal) @string

(line_str_text) @string
(multi_line_str_text) @string
(raw_str_part) @string
(raw_str_end_part) @string

[
  "\""
  "\"\"\""
] @string

(str_escaped_char) @escape

(regex_literal) @string

(line_string_literal
  [
    "\\("
    ")"
  ] @string.special)
(multi_line_string_literal
  [
    "\\("
    ")"
  ] @string.special)
(raw_str_interpolation
  [
    (raw_str_interpolation_start)
    ")"
  ] @string.special)

; ---------------------------------------------------------------------------
; Numbers, booleans, language constants
; ---------------------------------------------------------------------------
[
  (integer_literal)
  (hex_literal)
  (oct_literal)
  (bin_literal)
] @number
(real_literal) @number

(boolean_literal) @boolean

"nil" @constant.builtin
(special_literal) @constant.builtin
(wildcard_pattern) @constant.builtin

; ---------------------------------------------------------------------------
; Keywords (split: control-flow -> @keyword.control, rest -> @keyword)
; ---------------------------------------------------------------------------
[
  "func"
  "deinit"
  "protocol"
  "extension"
  "indirect"
  "nonisolated"
  "override"
  "convenience"
  "required"
  "some"
  "any"
  "weak"
  "unowned"
  "didSet"
  "willSet"
  "subscript"
  "let"
  "var"
  "enum"
  "struct"
  "class"
  "typealias"
  "async"
  "await"
  (throws)
  (where_keyword)
  (getter_specifier)
  (setter_specifier)
  (modify_specifier)
  (else)
  (as_operator)
] @keyword

(import_declaration
  "import" @keyword)

(shebang_line) @keyword
(directive) @keyword

; Control flow
[
  "while"
  "repeat"
  "continue"
  "break"
  "fallthrough"
] @keyword.control

(for_statement
  "for" @keyword.control)
(for_statement
  "in" @keyword.control)
(guard_statement
  "guard" @keyword.control)
(if_statement
  "if" @keyword.control)
(switch_statement
  "switch" @keyword.control)
(switch_entry
  "case" @keyword.control)
(switch_entry
  (default_keyword) @keyword.control)
(enum_entry
  "case" @keyword.control)
"return" @keyword.control

(ternary_expression
  [
    "?"
    ":"
  ] @keyword.control)

[
  (try_operator)
  "do"
  (throw_keyword)
  (catch_keyword)
] @keyword.control

(lambda_literal
  "in" @keyword.operator)

; Modifiers
[
  (visibility_modifier)
  (member_modifier)
  (function_modifier)
  (property_modifier)
  (parameter_modifier)
  (inheritance_modifier)
  (mutation_modifier)
] @keyword

; ---------------------------------------------------------------------------
; Identifiers, types, builtins (GENERIC — keep before specific overrides)
; ---------------------------------------------------------------------------
(simple_identifier) @variable
(type_identifier) @type

[
  (self_expression)
  (super_expression)
] @variable.builtin

; ---------------------------------------------------------------------------
; Declarations: functions, constructors, parameters, properties
; ---------------------------------------------------------------------------
(function_declaration
  (simple_identifier) @method)
(protocol_function_declaration
  name: (simple_identifier) @method)

(init_declaration
  "init" @constructor)

(parameter
  external_name: (simple_identifier) @parameter)
(parameter
  name: (simple_identifier) @parameter)
(type_parameter
  (type_identifier) @parameter)
(inheritance_constraint
  (identifier
    (simple_identifier) @parameter))
(equality_constraint
  (identifier
    (simple_identifier) @parameter))

(class_body
  (property_declaration
    (pattern
      (simple_identifier) @property)))
(protocol_property_declaration
  (pattern
    (simple_identifier) @property))
(navigation_expression
  (navigation_suffix
    (simple_identifier) @property))
(value_argument
  name: (value_argument_label
    (simple_identifier) @property))

; ---------------------------------------------------------------------------
; Attributes / macros
; ---------------------------------------------------------------------------
(modifiers
  (attribute
    "@" @attribute
    (user_type
      (type_identifier) @attribute)))

[
  (diagnostic)
  (availability_condition)
  (playground_literal)
  (key_path_string_expression)
  (selector_expression)
  (external_macro_definition)
] @macro

; ---------------------------------------------------------------------------
; Function / method calls
; ---------------------------------------------------------------------------
(call_expression
  (simple_identifier) @function) ; foo()
(call_expression
  (navigation_expression
    (navigation_suffix
      (simple_identifier) @method))) ; foo.bar.baz()
(call_expression
  (prefix_expression
    (simple_identifier) @function)) ; .foo()

((navigation_expression
  (simple_identifier) @type) ; SomeType.method(): highlight SomeType as a type
  (#match? @type "^[A-Z]"))

; ---------------------------------------------------------------------------
; Labels
; ---------------------------------------------------------------------------
(statement_label) @label

; ---------------------------------------------------------------------------
; Operators
; ---------------------------------------------------------------------------
(custom_operator) @operator

[
  "+"
  "-"
  "*"
  "/"
  "%"
  "="
  "+="
  "-="
  "*="
  "/="
  "%="
  "<"
  ">"
  "<<"
  ">>"
  "<="
  ">="
  "++"
  "--"
  "^"
  "&"
  "&&"
  "|"
  "||"
  "~"
  "!="
  "!=="
  "=="
  "==="
  "?"
  "??"
  "->"
  "..<"
  "..."
  (bang)
] @operator

; ---------------------------------------------------------------------------
; Punctuation
; ---------------------------------------------------------------------------
[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

(type_arguments
  [
    "<"
    ">"
  ] @punctuation.bracket)

[
  "."
  ";"
  ":"
  ","
  "#"
] @punctuation.delimiter
