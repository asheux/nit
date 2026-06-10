; TypeScript highlights (editor-grade) — JavaScript base + TS additions.
; Node names are taken from the upstream tree-sitter-javascript / -typescript
; highlight queries (JS nodes are valid against the TS superset grammar) and
; remapped to nit's supported capture names.

(comment) @comment

[
  (string)
  (template_string)
] @string
(regex) @string.special
(number) @number

[
  (true)
  (false)
  (null)
  (undefined)
] @constant.builtin

(this) @variable.builtin
(super) @variable.builtin

; Generic identifier base; specific roles below override it.
(identifier) @variable

; Types
(type_identifier) @type
(predefined_type) @type.builtin
((identifier) @type (#match? @type "^[A-Z]"))

(property_identifier) @property

; Functions / methods (definitions + calls)
(function_declaration name: (identifier) @function)
(function_expression name: (identifier) @function)
(method_definition name: (property_identifier) @method)
(call_expression function: (identifier) @function)
(call_expression function: (member_expression property: (property_identifier) @method))
(variable_declarator
  name: (identifier) @function
  value: [(function_expression) (arrow_function)])

; Parameters
(required_parameter (identifier) @parameter)
(optional_parameter (identifier) @parameter)

; SCREAMING_SNAKE_CASE constants (after the generic identifier rule so it wins).
((identifier) @constant (#match? @constant "^[A-Z_][A-Z0-9_]+$"))

; Control-flow keywords
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
  "throw"
  "try"
  "catch"
  "finally"
  "await"
  "yield"
] @keyword.control

; Declaration / modifier keywords (JS + TS)
[
  "as"
  "async"
  "class"
  "const"
  "debugger"
  "delete"
  "export"
  "extends"
  "from"
  "function"
  "get"
  "import"
  "in"
  "instanceof"
  "let"
  "new"
  "of"
  "set"
  "static"
  "target"
  "typeof"
  "var"
  "void"
  "with"
  "abstract"
  "declare"
  "enum"
  "implements"
  "interface"
  "keyof"
  "namespace"
  "private"
  "protected"
  "public"
  "readonly"
  "type"
  "override"
  "satisfies"
] @keyword

; Operators
[
  "-" "--" "-=" "+" "++" "+=" "*" "*=" "**" "**=" "/" "/=" "%" "%="
  "<" "<=" "<<" "<<=" "=" "==" "===" "!" "!=" "!==" "=>" ">" ">="
  ">>" ">>=" ">>>" ">>>=" "~" "^" "&" "|" "^=" "&=" "|=" "&&" "||"
  "??" "&&=" "||=" "??="
] @operator

["(" ")" "[" "]" "{" "}"] @punctuation.bracket
[";" "." "," ":"] @punctuation.delimiter
