; C++ highlights (editor-grade, full token taxonomy)
; Node names and anonymous tokens validated against tree-sitter-cpp 0.23.4
; (src/node-types.json). Generic captures come first; specific overrides last.

; ---------------------------------------------------------------------------
; Comments
; ---------------------------------------------------------------------------
(comment) @comment

; ---------------------------------------------------------------------------
; Strings / characters / escapes
; ---------------------------------------------------------------------------
(string_literal) @string
(raw_string_literal) @string
(system_lib_string) @string
(concatenated_string) @string
(char_literal) @character
(escape_sequence) @escape
(literal_suffix) @operator

; ---------------------------------------------------------------------------
; Numbers / booleans / language constants
; ---------------------------------------------------------------------------
(number_literal) @number

[
  (true)
  (false)
] @boolean

(null) @constant.builtin
(this) @variable.builtin

; ---------------------------------------------------------------------------
; Keywords — control flow
; ---------------------------------------------------------------------------
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
  "goto"
  "try"
  "catch"
  "throw"
  "co_await"
  "co_return"
  "co_yield"
] @keyword.control

; ---------------------------------------------------------------------------
; Keywords — declarations / specifiers / misc
; ---------------------------------------------------------------------------
[
  "typedef"
  "struct"
  "union"
  "enum"
  "class"
  "namespace"
  "template"
  "typename"
  "using"
  "public"
  "private"
  "protected"
  "static"
  "const"
  "constexpr"
  "consteval"
  "constinit"
  "extern"
  "explicit"
  "inline"
  "operator"
  "new"
  "delete"
  "sizeof"
  "friend"
  "mutable"
  "virtual"
  "override"
  "final"
  "noexcept"
  "register"
  "volatile"
  "concept"
  "requires"
  "decltype"
  "thread_local"
  "static_assert"
  "alignas"
  "alignof"
  "offsetof"
] @keyword

; ---------------------------------------------------------------------------
; Keyword operators (alphabetic alternative tokens)
; ---------------------------------------------------------------------------
[
  "and"
  "and_eq"
  "bitand"
  "bitor"
  "compl"
  "not"
  "not_eq"
  "or"
  "or_eq"
  "xor"
  "xor_eq"
] @keyword.operator

; ---------------------------------------------------------------------------
; Types
; ---------------------------------------------------------------------------
(primitive_type) @type.builtin
(sized_type_specifier) @type.builtin
(type_identifier) @type
(namespace_identifier) @namespace
(auto) @type.builtin
(statement_identifier) @label

((namespace_identifier) @type
  (#match? @type "^[A-Z]"))

; ---------------------------------------------------------------------------
; Properties / fields
; ---------------------------------------------------------------------------
(field_identifier) @property

; ---------------------------------------------------------------------------
; Parameters
; ---------------------------------------------------------------------------
(parameter_declaration
  declarator: (identifier) @parameter)
(optional_parameter_declaration
  declarator: (identifier) @parameter)
(parameter_declaration
  declarator: (reference_declarator (identifier) @parameter))
(parameter_declaration
  declarator: (pointer_declarator (identifier) @parameter))

; ---------------------------------------------------------------------------
; Namespaces / scopes
; ---------------------------------------------------------------------------
(qualified_identifier
  scope: (namespace_identifier) @namespace)
(namespace_definition
  name: (namespace_identifier) @namespace)
(nested_namespace_specifier
  (namespace_identifier) @namespace)
(using_declaration
  (qualified_identifier
    scope: (namespace_identifier) @namespace))

; ---------------------------------------------------------------------------
; Functions / methods
; ---------------------------------------------------------------------------
(function_declarator
  declarator: (identifier) @function)
(function_declarator
  declarator: (field_identifier) @method)
(function_declarator
  declarator: (qualified_identifier
    name: (identifier) @function))
(template_function
  name: (identifier) @function)
(template_method
  name: (field_identifier) @method)

(call_expression
  function: (identifier) @function)
(call_expression
  function: (qualified_identifier
    name: (identifier) @function))
(call_expression
  function: (field_expression
    field: (field_identifier) @method))
(call_expression
  function: (template_function
    name: (identifier) @function))

; ---------------------------------------------------------------------------
; Constructors (type-named call targets)
; ---------------------------------------------------------------------------
(new_expression
  type: (type_identifier) @constructor)
((call_expression
  function: (identifier) @constructor)
  (#match? @constructor "^[A-Z]"))

; ---------------------------------------------------------------------------
; Macros / preprocessor / attributes
; ---------------------------------------------------------------------------
(preproc_def
  name: (identifier) @macro)
(preproc_function_def
  name: (identifier) @macro)
(preproc_call
  directive: (preproc_directive) @macro)

(attribute
  name: (identifier) @attribute)
(attribute
  prefix: (identifier) @namespace)

[
  "#define"
  "#elif"
  "#elifdef"
  "#elifndef"
  "#else"
  "#endif"
  "#if"
  "#ifdef"
  "#ifndef"
  "#include"
] @keyword
(preproc_directive) @keyword

; ---------------------------------------------------------------------------
; Operators
; ---------------------------------------------------------------------------
[
  "+" "-" "*" "/" "%"
  "=" "==" "!=" "<" ">" "<=" ">=" "<=>"
  "&&" "||" "!"
  "&" "|" "^" "~" "<<" ">>"
  "+=" "-=" "*=" "/=" "%=" "&=" "|=" "^=" "<<=" ">>="
  "++" "--"
  "->" "->*" ".*"
  "?"
] @operator

; ---------------------------------------------------------------------------
; Punctuation — brackets
; ---------------------------------------------------------------------------
[
  "(" ")"
  "[" "]"
  "{" "}"
  "[[" "]]"
] @punctuation.bracket

; ---------------------------------------------------------------------------
; Punctuation — delimiters
; ---------------------------------------------------------------------------
[
  "," ";" ":" "::" "." "..."
] @punctuation.delimiter

; ---------------------------------------------------------------------------
; SCREAMING_SNAKE_CASE identifiers -> constants
; ---------------------------------------------------------------------------
((identifier) @constant
  (#match? @constant "^[A-Z][A-Z0-9_]+$"))
