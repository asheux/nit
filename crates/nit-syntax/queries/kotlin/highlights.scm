; Kotlin highlights (editor-grade, full token taxonomy)
; Grammar: tree-sitter-kotlin-ng 1.1.x
; Every named node and anonymous token below was confirmed against the
; grammar's src/node-types.json. Generic captures come first; specific
; overrides come last (tree-sitter: later patterns win for the same node).

; ---------------------------------------------------------------------------
; Comments
; ---------------------------------------------------------------------------
(line_comment) @comment
(block_comment) @comment

; KDoc is a plain block_comment in this grammar (no dedicated doc node);
; tag it as documentation when it opens with `/**`.
((block_comment) @comment.documentation
  (#match? @comment.documentation "^/[*][*]"))

; ---------------------------------------------------------------------------
; Strings / chars / escapes / interpolation
; ---------------------------------------------------------------------------
(string_literal) @string
(multiline_string_literal) @string
(string_content) @string
(character_literal) @character
(escape_sequence) @escape

; `$` / `${ ... }` interpolation markers read as special string tokens.
(interpolation ["${" "}"] @string.special)
"$" @string.special

; ---------------------------------------------------------------------------
; Numbers
; ---------------------------------------------------------------------------
(number_literal) @number
(float_literal) @number

; ---------------------------------------------------------------------------
; Keywords — control flow
; ---------------------------------------------------------------------------
[
  "if" "else" "when"
  "for" "while" "do"
  "return" "return@"
  "try" "catch" "finally" "throw"
] @keyword.control

; ---------------------------------------------------------------------------
; Keywords — everything else
; ---------------------------------------------------------------------------
[
  "fun" "val" "var"
  "class" "interface" "object" "companion" "init" "typealias"
  "import" "package"
  "by" "where" "dynamic"
  "in" "out" "as" "as?" "is"
  "get" "set"
  "public" "private" "protected" "internal"
  "abstract" "final" "open" "override" "lateinit" "const"
  "enum" "sealed" "annotation" "data" "inner" "value"
  "tailrec" "operator" "infix" "inline" "external" "suspend"
  "vararg" "noinline" "crossinline"
  "expect" "actual"
  "constructor"
  "field" "file" "property" "receiver" "param" "setparam" "delegate"
] @keyword

; `reified` is only reachable through this named node (no standalone token).
(reification_modifier) @keyword
(variance_modifier) @keyword

; `this` / `super` (and their labeled `this@` / `super@` forms).
[
  "this" "this@"
  "super" "super@"
] @variable.builtin

; ---------------------------------------------------------------------------
; Types
; ---------------------------------------------------------------------------
(user_type (identifier) @type)
(type_parameter (identifier) @type)
(type_constraint (identifier) @type)

; Common Kotlin builtin types (refinement over the generic @type above).
((user_type (identifier) @type.builtin)
  (#any-of? @type.builtin
    "Int" "Long" "Short" "Byte" "Float" "Double" "Boolean" "Char"
    "String" "CharSequence" "Unit" "Nothing" "Any"
    "Array" "List" "MutableList" "Set" "MutableSet" "Map" "MutableMap"
    "Collection" "Iterable" "Sequence" "Pair" "Triple"
    "UInt" "ULong" "UShort" "UByte" "Number"))

; ---------------------------------------------------------------------------
; Constructors (delegation / annotation construction call the type)
; ---------------------------------------------------------------------------
(constructor_invocation (user_type (identifier) @constructor))

; ---------------------------------------------------------------------------
; Functions / methods
; ---------------------------------------------------------------------------
; Definition.
(function_declaration name: (identifier) @function)

; Direct call: `foo(...)`.
(call_expression (identifier) @function)

; Method call via navigation: `obj.foo(...)`.
(call_expression (navigation_expression (identifier) @method))

; Infix call: `a shl b`.
(infix_expression (identifier) @method)

; Callable reference: `::foo`, `Type::bar`.
(callable_reference (identifier) @function)

; ---------------------------------------------------------------------------
; Annotations / use-site targets (decorators)
; ---------------------------------------------------------------------------
(annotation) @attribute
(file_annotation) @attribute
(use_site_target) @attribute

; ---------------------------------------------------------------------------
; Parameters / properties / namespaces
; ---------------------------------------------------------------------------
(parameter (identifier) @parameter)
(class_parameter (identifier) @parameter)

; Property / field access via navigation: `obj.field`.
(navigation_expression (identifier) @property)

; Named argument label: `foo(name = ...)`.
(value_argument (identifier) @property)

; Package / import path segments read as namespaces.
(package_header (qualified_identifier (identifier) @namespace))
(import (qualified_identifier (identifier) @namespace))

; ---------------------------------------------------------------------------
; Labels: `loop@`, `return@loop`
; ---------------------------------------------------------------------------
(label) @label

; ---------------------------------------------------------------------------
; Operators
; ---------------------------------------------------------------------------
[
  "+" "-" "*" "/" "%"
  "++" "--"
  "=" "==" "===" "!=" "!=="
  "<" ">" "<=" ">="
  "&&" "||" "!" "!!"
  "+=" "-=" "*=" "/=" "%="
  "?:" "?"
  ".." "..<"
  "&"
] @operator

; ---------------------------------------------------------------------------
; Punctuation
; ---------------------------------------------------------------------------
["(" ")" "{" "}" "[" "]"] @punctuation.bracket
["," ";" ":" "::" "->" "." "?." "@"] @punctuation.delimiter
