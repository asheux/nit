; OCaml highlights (editor-grade, full token taxonomy)
; Node names and anonymous tokens validated against tree-sitter-ocaml 0.25.0
; (the exact grammar version pinned in nit-syntax). Capture names are limited
; to nit's supported set; upstream's @function.method / @function.builtin /
; @module / @punctuation.special are remapped to @method / @function /
; @namespace / @punctuation.bracket respectively.

; Comments
;---------

[
  (comment)
  (line_number_directive)
  (directive)
  (shebang)
] @comment

; Strings / characters / escapes
;-------------------------------

(string) @string
(character) @character
(quoted_string) @string
(quoted_string (quoted_string_content) @string)
(escape_sequence) @escape
(conversion_specification) @string.special

; Numbers / booleans
;-------------------

[
  (number)
  (signed_number)
] @number

(boolean) @boolean

; Keywords
;---------

; Control flow
[
  "if"
  "then"
  "else"
  "match"
  "when"
  "for"
  "while"
  "do"
  "done"
  "to"
  "downto"
  "try"
] @keyword.control

; Declarations / structure / misc keywords
[
  "and"
  "as"
  "assert"
  "begin"
  "class"
  "constraint"
  "effect"
  "end"
  "exception"
  "external"
  "fun"
  "function"
  "functor"
  "in"
  "include"
  "inherit"
  "initializer"
  "lazy"
  "let"
  "method"
  "module"
  "mutable"
  "new"
  "nonrec"
  "object"
  "of"
  "open"
  "private"
  "rec"
  "sig"
  "struct"
  "type"
  "val"
  "virtual"
  "with"
] @keyword

; Variables / parameters
;-----------------------

[
  (value_name)
  (type_variable)
] @variable

(value_pattern) @variable.parameter

; Properties (record fields, object labels, instance variables)
;--------------------------------------------------------------

[
  (label_name)
  (field_name)
  (instance_variable_name)
] @property

; Modules / namespaces
;---------------------

[
  (module_name)
  (module_type_name)
] @namespace

; Types
;------

[
  (class_name)
  (class_type_name)
  (type_constructor)
] @type

(
  (type_constructor) @type.builtin
  (#match? @type.builtin "^(int|char|bytes|string|float|bool|unit|exn|array|list|option|int32|int64|nativeint|format6|lazy_t)$")
)

; Constructors / polymorphic variant tags
;----------------------------------------

[
  (constructor_name)
  (tag)
] @constructor

; Attributes
;-----------

(attribute_id) @attribute

; Functions / methods
;--------------------

(let_binding
  pattern: (value_name) @function
  (parameter))

(let_binding
  pattern: (value_name) @function
  body: [(fun_expression) (function_expression)])

(value_specification (value_name) @function)

(external (value_name) @function)

(method_name) @method

(application_expression
  function: (value_path (value_name) @function))

(infix_expression
  left: (value_path (value_name) @function)
  operator: (concat_operator) @operator
  (#eq? @operator "@@"))

(infix_expression
  operator: (rel_operator) @operator
  right: (value_path (value_name) @function)
  (#eq? @operator "|>"))

(
  (value_name) @function
  (#match? @function "^(raise(_notrace)?|failwith|invalid_arg)$")
)

; Operators
;----------

[
  (prefix_operator)
  (sign_operator)
  (pow_operator)
  (mult_operator)
  (add_operator)
  (concat_operator)
  (rel_operator)
  (and_operator)
  (or_operator)
  (assign_operator)
  (hash_operator)
  (indexing_operator)
  (let_operator)
  (let_and_operator)
  (match_operator)
] @operator

[
  "*"
  "#"
  "::"
  "<-"
  "="
  "|"
  "~"
  "?"
  "+"
  "-"
  "!"
  ">"
  "<"
  "&"
  "->"
  ":>"
  "+="
  ":="
  ".."
] @operator

; let-operators and match-operators used in binding/match positions read as keywords
(match_expression (match_operator) @keyword)
(value_definition [(let_operator) (let_and_operator)] @keyword)

; Brackets / delimiters
;----------------------

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
  "[|"
  "|]"
  "[<"
  "[>"
] @punctuation.bracket

(object_type ["<" ">"] @punctuation.bracket)

(attribute ["[@" "]"] @punctuation.bracket)
(item_attribute ["[@@" "]"] @punctuation.bracket)
(floating_attribute ["[@@@" "]"] @punctuation.bracket)
(extension ["[%" "]"] @punctuation.bracket)
(item_extension ["[%%" "]"] @punctuation.bracket)
(quoted_extension ["{%" "}"] @punctuation.bracket)
(quoted_item_extension ["{%%" "}"] @punctuation.bracket)

[
  ","
  "."
  ";"
  ":"
  ";;"
] @punctuation.delimiter

; Qualified module access (Module.value / Module.Sub.value)
;----------------------------------------------------------

(value_path (module_path (module_name) @namespace))
(field_path (module_path (module_name) @namespace))
(constructor_path (module_path (module_name) @namespace))
