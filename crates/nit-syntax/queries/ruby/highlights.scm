; Ruby highlights (editor-grade, full token taxonomy)
; Grammar: tree-sitter-ruby 0.23.1
; tree-sitter precedence: later patterns override earlier for the same node,
; so generic captures come first and specific overrides come last.

; ---------------------------------------------------------------------------
; Comments
; ---------------------------------------------------------------------------
(comment) @comment

; ---------------------------------------------------------------------------
; Literals: strings, symbols, regex, escapes, chars
; ---------------------------------------------------------------------------
[
  (string)
  (bare_string)
  (subshell)
  (string_array)
  (symbol_array)
  (chained_string)
  (heredoc_body)
  (heredoc_beginning)
] @string

(string_content) @string

[
  (simple_symbol)
  (delimited_symbol)
  (hash_key_symbol)
  (bare_symbol)
] @string.special

(regex) @string.special

(character) @character

(escape_sequence) @escape

(interpolation
  "#{" @punctuation.delimiter
  "}" @punctuation.delimiter)

; ---------------------------------------------------------------------------
; Numbers
; ---------------------------------------------------------------------------
[
  (integer)
  (float)
  (rational)
  (complex)
] @number

; ---------------------------------------------------------------------------
; Booleans & language constants
; ---------------------------------------------------------------------------
[
  (true)
  (false)
] @boolean

(nil) @constant.builtin

[
  (file)
  (line)
  (encoding)
] @constant.builtin

((identifier) @constant.builtin
 (#match? @constant.builtin "^__(FILE|LINE|ENCODING)__$"))

; ---------------------------------------------------------------------------
; Keywords — control flow vs. declaration/other
; ---------------------------------------------------------------------------
[
  "if"
  "elsif"
  "else"
  "unless"
  "case"
  "when"
  "then"
  "while"
  "until"
  "for"
  "in"
  "return"
  "yield"
  "break"
  "next"
  "redo"
  "retry"
  "begin"
  "rescue"
  "ensure"
] @keyword.control

[
  "def"
  "end"
  "class"
  "module"
  "do"
  "alias"
  "undef"
  "and"
  "or"
  "not"
  "defined?"
  "BEGIN"
  "END"
] @keyword

((identifier) @keyword
 (#match? @keyword "^(private|protected|public|require|require_relative|include|extend|attr_accessor|attr_reader|attr_writer)$"))

; ---------------------------------------------------------------------------
; Types & constructors
; ---------------------------------------------------------------------------
(constant) @type

(scope_resolution name: (constant) @type)

(class name: (constant) @constructor)
(class name: (scope_resolution name: (constant) @constructor))
(module name: (constant) @constructor)
(module name: (scope_resolution name: (constant) @constructor))
(superclass (constant) @type)

; ---------------------------------------------------------------------------
; Functions / methods
; ---------------------------------------------------------------------------
(method name: [(identifier) (constant)] @function)
(singleton_method name: [(identifier) (constant)] @function)
(alias (identifier) @function)
(setter (identifier) @function)

(call method: [(identifier) (constant)] @method)

; ---------------------------------------------------------------------------
; Parameters
; ---------------------------------------------------------------------------
(block_parameter (identifier) @parameter)
(block_parameters (identifier) @parameter)
(destructured_parameter (identifier) @parameter)
(hash_splat_parameter (identifier) @parameter)
(lambda_parameters (identifier) @parameter)
(method_parameters (identifier) @parameter)
(splat_parameter (identifier) @parameter)
(keyword_parameter name: (identifier) @parameter)
(optional_parameter name: (identifier) @parameter)

; ---------------------------------------------------------------------------
; Variables & properties
; ---------------------------------------------------------------------------
(identifier) @variable

[
  (self)
  (super)
] @variable.builtin

(instance_variable) @property
(class_variable) @property
(global_variable) @variable.builtin

(pair key: (hash_key_symbol) @property)
(call receiver: (_) method: (identifier) @property
 (#match? @property "^[a-z_][a-zA-Z0-9_]*$"))

; ---------------------------------------------------------------------------
; Operators
; ---------------------------------------------------------------------------
[
  "+" "-" "*" "/" "%" "**"
  "=" "==" "===" "!=" "<" ">" "<=" ">=" "<=>"
  "&&" "||" "!"
  "&" "|" "^" "~" "<<" ">>"
  "=~" "!~"
  "+=" "-=" "*=" "/=" "%=" "**="
  "&=" "|=" "^=" "<<=" ">>=" "&&=" "||="
  "->" "=>" ".." "..." "?" ":"
  "&." "+@" "-@" "~@"
] @operator

; ---------------------------------------------------------------------------
; Punctuation
; ---------------------------------------------------------------------------
[
  "(" ")" "[" "]" "{" "}" "%w(" "%i("
] @punctuation.bracket

[
  "," ";" "." "::"
] @punctuation.delimiter

; ---------------------------------------------------------------------------
; ALL-CAPS constants override (after @type / @constructor so it wins)
; ---------------------------------------------------------------------------
((constant) @constant
 (#match? @constant "^[A-Z][A-Z0-9_]*$"))
