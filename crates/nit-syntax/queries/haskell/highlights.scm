; Haskell highlights (editor-grade, full token taxonomy)
; Node names and anonymous tokens verified against tree-sitter-haskell 0.23.1
; (src/node-types.json + upstream queries/highlights.scm).
;
; Precedence note: tree-sitter applies the LAST matching pattern for a node.
; Generic captures come first (variable/type), specific overrides last
; (function/constructor/property/boolean), so the specific colors win.

; ----------------------------------------------------------------------------
; Parameters and variables (low priority — kept first so destructured
; parameters and later overrides take precedence)
(variable) @variable

(wildcard) @variable

(decl/function
  patterns: (patterns
    (_) @parameter))

(expression/lambda
  (_)+ @parameter
  "->")

; ----------------------------------------------------------------------------
; Literals and comments
(integer) @number
(float) @number
(negation) @number

(char) @character
(string) @string

(unit) @string.special ; the () unit value

(comment) @comment
((haddock) @comment.documentation)

; ----------------------------------------------------------------------------
; Types and constructors
(name) @type
(type/star) @type

(constructor) @constructor

; ----------------------------------------------------------------------------
; Modules / namespaces
(module
  (module_id) @namespace)

; ----------------------------------------------------------------------------
; Pragmas / annotations
(pragma) @attribute

; ----------------------------------------------------------------------------
; Keywords — control flow first (@keyword.control), the rest @keyword
[
  "if"
  "then"
  "else"
  "case"
  "of"
] @keyword.control

[
  "module"
  "import"
  "qualified"
  "hiding"
  "as"
  "where"
  "let"
  "in"
  "class"
  "instance"
  "pattern"
  "data"
  "newtype"
  "family"
  "type"
  "deriving"
  "via"
  "stock"
  "anyclass"
  "do"
  "mdo"
  "rec"
  "forall"
  "foreign"
  "export"
  "default"
  "role"
  "infix"
  "infixl"
  "infixr"
] @keyword

; ----------------------------------------------------------------------------
; Operators
[
  (operator)
  (constructor_operator)
  (all_names)
  "="
  "|"
  ".."
  "::"
  "=>"
  "->"
  "<-"
  "\\"
  "`"
  "@"
  "!"
  "~"
  "$"
  "%"
  "||"
] @operator

; ----------------------------------------------------------------------------
; Punctuation
[
  "("
  ")"
  "{"
  "}"
  "["
  "]"
] @punctuation.bracket

[
  ","
  ";"
  "."
] @punctuation.delimiter

; ----------------------------------------------------------------------------
; Functions (specific overrides — placed after generic (variable) @variable)
(decl
  [
    name: (variable) @function
    names: (binding_list (variable) @function)
  ])

(decl/function
  name: (variable) @function)

(decl/signature
  name: (variable) @function
  type: (quantified_type))

; function applications / calls
(apply
  [
    (expression/variable) @function
    (expression/qualified
      (variable) @function)
  ])

(view_pattern
  [
    (expression/variable) @function
    (expression/qualified
      (variable) @function)
  ])

; quasi-quoter name
(quoter) @function

; main is always a function
(decl/bind
  name: (variable) @function
  (#eq? @function "main"))

; ----------------------------------------------------------------------------
; Fields / record selectors
(field_name
  (variable) @property)

; ----------------------------------------------------------------------------
; Booleans (override constructors/variables — placed last)
((constructor) @boolean
  (#any-of? @boolean "True" "False"))

((variable) @boolean
  (#eq? @boolean "otherwise"))
