; C highlights (editor-grade, full token taxonomy)

; --- Comments ---
(comment) @comment

; --- Strings / chars / escapes ---
(string_literal) @string
(system_lib_string) @string
(concatenated_string) @string
(char_literal) @character
(escape_sequence) @escape

; --- Numbers ---
(number_literal) @number

; --- Booleans / language constants ---
(true) @boolean
(false) @boolean
(null) @constant.builtin
"NULL" @constant.builtin
"nullptr" @constant.builtin

; --- Keywords: control flow ---
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
] @keyword.control

; --- Keywords: everything else ---
[
  "sizeof"
  "offsetof"
  "alignof"
  "_Alignof"
  "__alignof"
  "__alignof__"
  "_alignof"
  "alignas"
  "_Alignas"
  "typedef"
  "struct"
  "union"
  "enum"
  "static"
  "const"
  "constexpr"
  "extern"
  "register"
  "auto"
  "volatile"
  "__volatile__"
  "restrict"
  "__restrict__"
  "inline"
  "__inline"
  "__inline__"
  "__forceinline"
  "_Atomic"
  "_Noreturn"
  "noreturn"
  "thread_local"
  "__thread"
  "_Generic"
  "_Nonnull"
  "defined"
  "asm"
  "__asm"
  "__asm__"
  "__attribute"
  "__attribute__"
  "__declspec"
  "__extension__"
  "__based"
  "__cdecl"
  "__clrcall"
  "__stdcall"
  "__fastcall"
  "__thiscall"
  "__vectorcall"
  "__try"
  "__except"
  "__finally"
  "__leave"
  "__unaligned"
  "_unaligned"
] @keyword

(storage_class_specifier) @keyword
(type_qualifier) @keyword

; --- Preprocessor directives ---
"#define" @keyword
"#elif" @keyword
"#elifdef" @keyword
"#elifndef" @keyword
"#else" @keyword
"#endif" @keyword
"#if" @keyword
"#ifdef" @keyword
"#ifndef" @keyword
"#include" @keyword
(preproc_directive) @keyword

; --- Types ---
(primitive_type) @type.builtin
(sized_type_specifier) @type.builtin
(type_identifier) @type
[
  "long"
  "short"
  "signed"
  "unsigned"
] @type.builtin

; --- Functions / methods / macros ---
(function_declarator declarator: (identifier) @function)
(call_expression function: (identifier) @function)
(call_expression function: (field_expression field: (field_identifier) @method))

(preproc_def name: (identifier) @macro)
(preproc_function_def name: (identifier) @macro)
(preproc_call directive: (preproc_directive) @macro)

; --- Attributes ---
(attribute name: (identifier) @attribute)
(attribute prefix: (identifier) @attribute)

; --- Parameters ---
(parameter_declaration declarator: (identifier) @parameter)
(parameter_declaration declarator: (pointer_declarator declarator: (identifier) @parameter))
(parameter_declaration declarator: (pointer_declarator declarator: (pointer_declarator declarator: (identifier) @parameter)))

; --- Fields / properties ---
(field_identifier) @property
(field_designator (field_identifier) @property)

; --- Enum constants ---
(enumerator name: (identifier) @constant)

; --- Labels ---
(statement_identifier) @label

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
  "->"
  "?"
] @operator

; --- Brackets ---
[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
  "[["
  "]]"
] @punctuation.bracket

; --- Delimiters ---
[
  ","
  ";"
  ":"
  "::"
  "."
  "..."
] @punctuation.delimiter

; --- Constant-cased identifiers (last so it overrides @variable defaults) ---
((identifier) @constant
 (#match? @constant "^[A-Z][A-Z0-9_]*$"))
