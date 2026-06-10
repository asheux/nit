; Java highlights (editor-grade, full token taxonomy)
; Built against tree-sitter-java 0.23.5 — node names/tokens verified against
; the grammar's own node-types.json, grammar.json, and queries/highlights.scm.

; --- Variables (generic first; specific captures below override) ---
(identifier) @variable

(this) @variable.builtin
(super) @variable.builtin

; --- Comments ---
[
  (line_comment)
  (block_comment)
] @comment

; --- Literals: strings / chars / escapes ---
[
  (string_literal)
  (character_literal)
] @string

(escape_sequence) @escape

; --- Numbers ---
[
  (decimal_integer_literal)
  (hex_integer_literal)
  (octal_integer_literal)
  (binary_integer_literal)
  (decimal_floating_point_literal)
  (hex_floating_point_literal)
] @number

; --- Booleans / language constants ---
[
  (true)
  (false)
] @boolean

(null_literal) @constant.builtin

; --- Keywords: control-flow ---
[
  "if"
  "else"
  "for"
  "while"
  "do"
  "switch"
  "case"
  "default"
  "break"
  "continue"
  "return"
  "yield"
  "try"
  "catch"
  "finally"
  "throw"
  "throws"
  "assert"
  "when"
] @keyword.control

; --- Keywords: declarations / modifiers / module system / misc ---
[
  "abstract"
  "class"
  "interface"
  "enum"
  "record"
  "extends"
  "implements"
  "permits"
  "sealed"
  "non-sealed"
  "public"
  "private"
  "protected"
  "static"
  "final"
  "synchronized"
  "volatile"
  "transient"
  "native"
  "strictfp"
  "new"
  "instanceof"
  "import"
  "package"
  "module"
  "open"
  "opens"
  "requires"
  "exports"
  "provides"
  "uses"
  "to"
  "with"
  "transitive"
] @keyword

; --- Types ---
(type_identifier) @type

[
  (boolean_type)
  (integral_type)
  (floating_point_type)
  (void_type)
] @type.builtin

; Capitalized identifiers used as a scope/object are almost always types.
((field_access
  object: (identifier) @type)
 (#match? @type "^[A-Z]"))
((scoped_identifier
  scope: (identifier) @type)
 (#match? @type "^[A-Z]"))
((method_invocation
  object: (identifier) @type)
 (#match? @type "^[A-Z]"))

; --- Constructors ---
(constructor_declaration
  name: (identifier) @constructor)
(object_creation_expression
  type: (type_identifier) @constructor)
(explicit_constructor_invocation
  constructor: (this) @constructor)
(explicit_constructor_invocation
  constructor: (super) @constructor)

; --- Methods (definitions and calls) ---
(method_declaration
  name: (identifier) @method)
(method_invocation
  name: (identifier) @method)
(method_reference
  (identifier) @method)

; --- Annotations / attributes ---
(annotation
  name: (identifier) @attribute)
(annotation
  name: (scoped_identifier) @attribute)
(marker_annotation
  name: (identifier) @attribute)
(marker_annotation
  name: (scoped_identifier) @attribute)

; --- Parameters ---
(formal_parameter
  name: (identifier) @parameter)
(catch_formal_parameter
  name: (identifier) @parameter)
(spread_parameter
  (variable_declarator
    name: (identifier) @parameter))
(lambda_expression
  parameters: (identifier) @parameter)
(inferred_parameters
  (identifier) @parameter)

; --- Properties / fields ---
(field_access
  field: (identifier) @property)

; --- Namespaces / packages ---
(package_declaration
  (identifier) @namespace)
(package_declaration
  (scoped_identifier) @namespace)

; --- Constants (ALL_CAPS identifiers) ---
((identifier) @constant
 (#match? @constant "^_*[A-Z][A-Z\\d_]+$"))

; --- Operators ---
[
  "+"
  "-"
  "*"
  "/"
  "%"
  "++"
  "--"
  "="
  "=="
  "!="
  "<"
  ">"
  "<="
  ">="
  "&&"
  "||"
  "!"
  "&"
  "|"
  "^"
  "~"
  "<<"
  ">>"
  ">>>"
  "+="
  "-="
  "*="
  "/="
  "%="
  "&="
  "|="
  "^="
  "<<="
  ">>="
  ">>>="
  "->"
  "@"
] @operator

; --- Punctuation: brackets ---
[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

; --- Punctuation: delimiters ---
[
  ","
  ";"
  ":"
  "::"
  "."
  "..."
  "?"
] @punctuation.delimiter
