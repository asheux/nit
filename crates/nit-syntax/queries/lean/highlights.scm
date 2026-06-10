; Lean 4 highlights (tree-sitter-lean4 0.3) — editor-grade, full token taxonomy.
; Every named node and anonymous token below was confirmed against the
; grammar's src/node-types.json and grammar.js. Generic captures come first;
; field-scoped specific captures come last (tree-sitter: later pattern wins).

; ── Comments ──────────────────────────────────────────────────────────────
; `comment` covers both `-- line` and `/- block -/`; doc variants `/-- -/`
; and `/-! -/` are lexed by the same `comment` rule (no separate node).
(comment) @comment

; ── Literals ──────────────────────────────────────────────────────────────
(string) @string
(interpolated_string) @string
(escape_sequence) @escape
(char) @character

(number) @number
(float) @number

; Name-quotation atoms: `` `name ``, ``` ``name ```, `` `(...) ``
(quoted_name) @string.special
(double_quoted_name) @string.special
(syntax_quotation) @string.special

; ── Keywords (original set — kept verbatim, proven valid) ──────────────────
[
  "def"
  "theorem"
  "lemma"
  "example"
  "axiom"
  "structure"
  "class"
  "instance"
  "inductive"
  "abbrev"
  "opaque"
  "noncomputable"
  "partial"
  "private"
  "protected"
  "open"
  "namespace"
  "end"
  "section"
  "variable"
  "universe"
  "import"
  "if"
  "then"
  "else"
  "match"
  "with"
  "do"
  "let"
  "fun"
  "in"
  "where"
  "by"
  "have"
  "show"
  "return"
  "deriving"
] @keyword

; ── Keywords (additional declaration / modifier / contextual tokens) ───────
[
  "constant"
  "universes"
  "export"
  "include"
  "omit"
  "attribute"
  "set_option"
  "initialize"
  "builtin_initialize"
  "extends"
  "mut"
  "scoped"
  "local"
  "meta"
  "public"
  "unsafe"
  "hiding"
  "at"
  "calc"
  "case"
  "next"
  "obtain"
  "suffices"
  "dbg_trace"
  "λ"
  "forall"
  "exists"
  "∀"
  "∃"
  ; notation / macro declaration heads
  "notation"
  "macro"
  "macro_rules"
  "elab"
  "syntax"
  "prefix"
  "infix"
  "infixl"
  "infixr"
  "postfix"
] @keyword

; `module` / `prelude` are whole-rule string tokens promoted to named nodes
; (no standalone anonymous token exists), so capture the node, not the string.
(module_header) @keyword
(prelude) @keyword

; ── Control-flow keywords (override the originals above where they overlap) ─
[
  "if"
  "then"
  "else"
  "match"
  "with"
  "do"
  "return"
  "for"
  "while"
  "try"
  "catch"
  "unless"
] @keyword.control

; `break` / `continue` are likewise named nodes wrapping a single token.
(do_break) @keyword.control
(do_continue) @keyword.control

; ── Booleans & builtin constants ──────────────────────────────────────────
(true) @boolean
(false) @boolean
(sorry) @constant.builtin

; Placeholders / holes
(hole) @variable.builtin
(synthetic_hole) @variable.builtin
(cdot) @variable.builtin

; ── Operators ─────────────────────────────────────────────────────────────
[
  "+" "-" "*" "/" "%" "++" "::" "^" "∘" "×" "∪" "∩" "\\"
  "&&" "||" "∧" "∨"
  "==" "!=" "=" "<" ">" "<=" ">=" "≤" "≥" "≠" "∣" "↔" "⊢"
  "!" "¬"
  "->" "→"
  "←" "<-"
  "$" "<|" "<|>" "<$>" "<*>" "*>" "<*" "|>" "|>."
  "@"
] @operator

; Postfix/anonymous operator-like atoms exposed as named nodes
(ellipsis) @operator

; ── Brackets & delimiters ─────────────────────────────────────────────────
["(" ")" "[" "]" "{" "}" "#[" "⟨" "⟩"] @punctuation.bracket
["," ";" ":" "::" "." ":=" "=>" "|"] @punctuation.delimiter

; ── Types ─────────────────────────────────────────────────────────────────
; `type` is a labeled field on binders, type-specs, and structure fields.
(instance_binder type: (identifier) @type)
(explicit_binder type: (identifier) @type)
(implicit_binder type: (identifier) @type)
(structure_field type: (identifier) @type)

; ── Constructors ──────────────────────────────────────────────────────────
(constructor name: (identifier) @constructor)
(constructor_pattern constructor: (identifier) @constructor)
(anonymous_constructor) @constructor

; ── Functions / methods ───────────────────────────────────────────────────
(definition name: (identifier) @function)
(where_decl name: (identifier) @method)
(application name: (identifier) @function)

; ── Attributes / macros ───────────────────────────────────────────────────
(attribute_entry name: (identifier) @attribute)

; ── Namespaces / modules ──────────────────────────────────────────────────
(namespace name: (identifier) @namespace)
(section name: (identifier) @namespace)
(end name: (identifier) @namespace)
(import module: (identifier) @namespace)
(open namespace: (identifier) @namespace)
(export class: (identifier) @namespace)

; ── Parameters ────────────────────────────────────────────────────────────
(explicit_binder name: (identifier) @parameter)
(implicit_binder name: (identifier) @parameter)
(instance_binder name: (identifier) @parameter)

; ── Properties / fields / projections ─────────────────────────────────────
(structure_field name: (identifier) @property)
(field_assignment name: (identifier) @property)
(projection name: (identifier) @property)
