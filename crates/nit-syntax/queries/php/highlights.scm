; PHP highlights (editor-grade, full token taxonomy)
; Grammar: tree-sitter-php 0.24 (LANGUAGE_PHP variant).
; Generic captures first, specific overrides last (later patterns win).

; --- Comments -------------------------------------------------------------
(comment) @comment

; --- Strings & escapes ----------------------------------------------------
[
  (string)
  (string_content)
  (encapsed_string)
  (heredoc)
  (heredoc_body)
  (nowdoc)
  (nowdoc_body)
] @string

(shell_command_expression) @string.special

(escape_sequence) @escape

; --- Numbers / booleans / language constants ------------------------------
(integer) @number
(float) @number

(boolean) @constant.builtin
(null) @constant.builtin

; --- PHP tags -------------------------------------------------------------
[
  (php_tag)
  (php_end_tag)
] @tag

; --- Keywords -------------------------------------------------------------
; Control flow
[
  "if"
  "elseif"
  "else"
  "endif"
  "for"
  "endfor"
  "foreach"
  "endforeach"
  "while"
  "endwhile"
  "do"
  "switch"
  "endswitch"
  "case"
  "default"
  "match"
  "break"
  "continue"
  "return"
  "yield"
  "yield from"
  "goto"
  "try"
  "catch"
  "finally"
  "throw"
] @keyword.control

; Declarations / modifiers / misc keywords
[
  "function"
  "fn"
  "class"
  "interface"
  "trait"
  "enum"
  "namespace"
  "use"
  "as"
  "insteadof"
  "extends"
  "implements"
  "new"
  "clone"
  "instanceof"
  "const"
  "global"
  "static"
  "abstract"
  "final"
  "readonly"
  "public"
  "private"
  "protected"
  "declare"
  "enddeclare"
  "echo"
  "print"
  "exit"
  "unset"
  "include"
  "include_once"
  "require"
  "require_once"
  "and"
  "or"
  "xor"
  (abstract_modifier)
  (final_modifier)
  (readonly_modifier)
  (static_modifier)
  (visibility_modifier)
  (var_modifier)
] @keyword

; --- Types ----------------------------------------------------------------
(primitive_type) @type.builtin
(cast_type) @type.builtin

(named_type
  [
    (name) @type
    (qualified_name (name) @type)
    (relative_name (name) @type)
  ])

(base_clause
  [
    (name) @type
    (qualified_name (name) @type)
    (relative_name (name) @type)
  ])

(class_interface_clause
  [
    (name) @type
    (qualified_name (name) @type)
    (relative_name (name) @type)
  ])

(scoped_call_expression
  scope: [
    (name) @type
    (qualified_name (name) @type)
    (relative_name (name) @type)
  ])

(class_constant_access_expression
  [
    (name) @type
    (qualified_name (name) @type)
    (relative_name (name) @type)
  ])

; --- Constructors ---------------------------------------------------------
(object_creation_expression
  [
    (name) @constructor
    (qualified_name (name) @constructor)
    (relative_name (name) @constructor)
  ])

(method_declaration
  name: (name) @constructor
  (#eq? @constructor "__construct"))

; --- Functions & methods --------------------------------------------------
(function_definition
  name: (name) @function)

(function_call_expression
  function: [
    (name)
    (qualified_name (name))
    (relative_name (name))
  ] @function)

(scoped_call_expression
  name: (name) @method)

(member_call_expression
  name: (name) @method)

(nullsafe_member_call_expression
  name: (name) @method)

(method_declaration
  name: (name) @method)

; --- Attributes (PHP 8 #[Attr]) ------------------------------------------
(attribute
  [
    (name) @attribute
    (qualified_name (name) @attribute)
    (relative_name (name) @attribute)
  ])

; --- Parameters -----------------------------------------------------------
(simple_parameter
  name: (variable_name) @parameter)
(variadic_parameter
  name: (variable_name) @parameter)
(property_promotion_parameter
  name: (variable_name) @parameter)

; --- Properties / fields --------------------------------------------------
(property_element
  (variable_name) @property)

(member_access_expression
  name: (name) @property)
(nullsafe_member_access_expression
  name: (name) @property)

; --- Namespaces -----------------------------------------------------------
(namespace_definition
  name: (namespace_name (name) @namespace))
(namespace_name (name) @namespace)
(relative_name "namespace" @namespace)

; --- Labels ---------------------------------------------------------------
(named_label_statement (name) @label)
(goto_statement (name) @label)

; --- Variables ------------------------------------------------------------
(variable_name) @variable
(relative_scope) @variable.builtin

((name) @variable.builtin
  (#eq? @variable.builtin "this"))

; --- Constants ------------------------------------------------------------
(const_declaration (const_element (name) @constant))
(enum_case name: (name) @constant)

((name) @constant
  (#match? @constant "^_?[A-Z][A-Z0-9_]+$"))
((name) @constant.builtin
  (#match? @constant.builtin "^__[A-Z][A-Z0-9_]+__$"))

; --- Operators ------------------------------------------------------------
[
  "+" "-" "*" "/" "%" "**"
  "=" "==" "===" "!=" "!==" "<>" "<" ">" "<=" ">=" "<=>"
  "&&" "||" "!"
  "&" "|" "^" "~" "<<" ">>"
  "+=" "-=" "*=" "/=" "%=" "**=" ".=" "&=" "|=" "^=" "<<=" ">>="
  "??" "??=" "?->" "->" "?" "." "..." "@" "$" "|>"
] @operator

; --- Brackets -------------------------------------------------------------
[
  "(" ")"
  "{" "}"
  "[" "]"
] @punctuation.bracket

; --- Delimiters -----------------------------------------------------------
[
  ","
  ";"
  ":"
  "::"
  "=>"
  "\\"
] @punctuation.delimiter
