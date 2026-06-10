; Makefile highlights (editor-grade, full token taxonomy)
; Node names + anonymous tokens verified against tree-sitter-make 1.1.1
; (src/node-types.json + grammar.js). nit-unsupported captures from the
; upstream query (@conditional/@repeat/@include/@exception/@text.*/
; @constant.macro/@punctuation.special) are remapped onto nit's supported set.

; --- Comments ---------------------------------------------------------------
(comment) @comment

; --- Strings / text / escapes ----------------------------------------------
(string) @string
(text) @string
(raw_text) @string
(shell_command) @string
(variable_assignment (word) @string)

(escape) @escape

; --- Variables --------------------------------------------------------------
(variable_reference) @variable
(substitution_reference) @variable
(automatic_variable) @variable.builtin

; --- Functions --------------------------------------------------------------
; Builtin GNU make functions, e.g. $(subst ...), $(wildcard ...), $(shell ...)
(function_call
  function: _ @function)
(shell_function
  function: _ @function)

; --- Directive keywords -----------------------------------------------------
; Control-flow conditionals
[
  "ifeq"
  "ifneq"
  "ifdef"
  "ifndef"
  "else"
  "endif"
] @keyword.control

; Other directives / declarations
[
  "include"
  "sinclude"
  "-include"
  "define"
  "endef"
  "override"
  "export"
  "unexport"
  "vpath"
  "undefine"
  "private"
] @keyword

; --- Operators --------------------------------------------------------------
; Assignment + recipe-line prefixes (verified anonymous tokens)
[
  "="
  ":="
  "::="
  "?="
  "+="
  "!="
  "@"
  "-"
  "+"
] @operator

; --- Punctuation: brackets --------------------------------------------------
[
  "("
  ")"
  "{"
  "}"
] @punctuation.bracket

; --- Punctuation: delimiters ------------------------------------------------
; Rule/target separators, list/path separators, quotes, comma
[
  ":"
  "&:"
  "::"
  "|"
  ";"
  ","
  "\""
  "'"
] @punctuation.delimiter

; Variable/function expansion sigils
[
  "$"
  "$$"
] @punctuation

; --- Targets ----------------------------------------------------------------
; Generic target name (original capture — kept).
(targets (word) @function)

; Standard / builtin special targets override the generic @function above.
(targets
  (word) @constant.builtin
  (#match? @constant.builtin "^(all|install|install-html|install-dvi|install-pdf|install-ps|uninstall|install-strip|clean|distclean|mostlyclean|maintainer-clean|TAGS|info|dvi|html|pdf|ps|dist|check|installcheck|installdirs)$"))

(targets
  (word) @constant.builtin
  (#match? @constant.builtin "^\\.(PHONY|SUFFIXES|DEFAULT|PRECIOUS|INTERMEDIATE|SECONDARY|SECONDEXPANSION|DELETE_ON_ERROR|IGNORE|LOW_RESOLUTION_TIME|SILENT|EXPORT_ALL_VARIABLES|NOTPARALLEL|ONESHELL|POSIX)$"))

; --- Constants (variable names) --------------------------------------------
(variable_assignment
  name: (word) @constant)

; Implicit-rule / well-known builtin variables override the generic @constant.
[
  "VPATH"
  ".RECIPEPREFIX"
] @constant.builtin

(variable_assignment
  name: (word) @constant.builtin
  (#match? @constant.builtin "^(AR|AS|CC|CXX|CPP|FC|M2C|PC|CO|GET|LEX|YACC|LINT|MAKEINFO|TEX|TEXI2DVI|WEAVE|CWEAVE|TANGLE|CTANGLE|RM|ARFLAGS|ASFLAGS|CFLAGS|CXXFLAGS|COFLAGS|CPPFLAGS|FFLAGS|GFLAGS|LDFLAGS|LDLIBS|LFLAGS|YFLAGS|PFLAGS|RFLAGS|LINTFLAGS|PRE_INSTALL|POST_INSTALL|NORMAL_INSTALL|PRE_UNINSTALL|POST_UNINSTALL|NORMAL_UNINSTALL|MAKEFILE_LIST|MAKE_RESTARTS|MAKE_TERMOUT|MAKE_TERMERR)$"))

(variable_reference
  (word) @constant.builtin
  (#match? @constant.builtin "^(AR|AS|CC|CXX|CPP|FC|M2C|PC|CO|GET|LEX|YACC|LINT|MAKEINFO|TEX|TEXI2DVI|WEAVE|CWEAVE|TANGLE|CTANGLE|RM|ARFLAGS|ASFLAGS|CFLAGS|CXXFLAGS|COFLAGS|CPPFLAGS|FFLAGS|GFLAGS|LDFLAGS|LDLIBS|LFLAGS|YFLAGS|PFLAGS|RFLAGS|LINTFLAGS|PRE_INSTALL|POST_INSTALL|NORMAL_INSTALL|PRE_UNINSTALL|POST_UNINSTALL|NORMAL_UNINSTALL|MAKEFILE_LIST|MAKE_RESTARTS|MAKE_TERMOUT|MAKE_TERMERR|\\.DEFAULT_GOAL|\\.RECIPEPREFIX|\\.EXTRA_PREREQS|\\.VARIABLES|\\.FEATURES|\\.INCLUDE_DIRS|\\.LOADED)$"))
