; Nix highlights (editor-grade, full token taxonomy)
; Node names + anonymous tokens verified against tree-sitter-nix 0.0.2
; (grammar.js, src/node-types.json, upstream queries/highlights.scm).

; --- Comments ---------------------------------------------------------------
(comment) @comment

; --- Strings, paths, escapes ------------------------------------------------
(string_expression) @string
(indented_string_expression) @string

[
  (path_expression)
  (hpath_expression)
  (spath_expression)
] @string.special

(uri_expression) @string.special

(escape_sequence) @escape
(dollar_escape) @escape

; --- Numbers ----------------------------------------------------------------
(integer_expression) @number
(float_expression) @number

; --- Keywords ---------------------------------------------------------------
; Control-flow subset.
[
  "if"
  "then"
  "else"
  "assert"
] @keyword.control

; Remaining keywords.
[
  "let"
  "in"
  "with"
  "rec"
  "inherit"
  "or"
] @keyword

; --- Generic identifier (lowest precedence; specific rules below override) ---
(variable_expression (identifier) @variable)

; --- Functions / application ------------------------------------------------
; A lambda's `arg: body` binder and `{ ... } @ name` binder are parameters.
(function_expression
  universal: (identifier) @parameter)

; Formal parameters in `{ a, b ? default }: ...`.
(formal
  name: (identifier) @parameter)

; Function being applied: `f x` or `pkgs.lib.foo x`.
(apply_expression
  function: [
    (variable_expression (identifier)) @function
    (select_expression
      attrpath: (attrpath
        attr: (identifier) @function .))])

; --- Attribute access / properties ------------------------------------------
; `a.b.c` selection path.
(select_expression
  attrpath: (attrpath (identifier)) @property)

; `attr = value;` binding name.
(binding
  attrpath: (attrpath (identifier)) @property)

; `inherit (x) a b;` inherited names.
(inherit_from
  attrs: (inherited_attrs attr: (identifier) @property))

; --- Operators --------------------------------------------------------------
; All tokens confirmed in grammar.js (unary/binary/has_attr expressions).
[
  "!"
  "=="
  "!="
  "<"
  "<="
  ">"
  ">="
  "&&"
  "||"
  "+"
  "-"
  "*"
  "/"
  "->"
  "//"
  "++"
  "?"
] @operator

; --- Punctuation ------------------------------------------------------------
[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

[
  ";"
  "."
  ","
  "="
  ":"
  "@"
  "${"
  (ellipses)
] @punctuation.delimiter

; --- Builtin constants / variables / functions ------------------------------
; Nix exposes these as plain identifiers, not dedicated grammar nodes, so they
; are matched by name and placed AFTER the generic identifier rule so the more
; specific capture wins (tree-sitter: later pattern wins for the same node).
((variable_expression (identifier) @boolean)
 (#match? @boolean "^(true|false)$"))

((variable_expression (identifier) @constant.builtin)
 (#eq? @constant.builtin "null"))

((variable_expression (identifier) @variable.builtin)
 (#match? @variable.builtin "^(__currentSystem|__currentTime|__nixPath|__nixVersion|__storeDir|builtins)$"))

((variable_expression (identifier) @function.builtin)
 (#match? @function.builtin "^(__add|__addErrorContext|__all|__any|__appendContext|__attrNames|__attrValues|__bitAnd|__bitOr|__bitXor|__catAttrs|__compareVersions|__concatLists|__concatMap|__concatStringsSep|__deepSeq|__div|__elem|__elemAt|__fetchurl|__filter|__filterSource|__findFile|__foldl'|__fromJSON|__functionArgs|__genList|__genericClosure|__getAttr|__getContext|__getEnv|__hasAttr|__hasContext|__hashFile|__hashString|__head|__intersectAttrs|__isAttrs|__isBool|__isFloat|__isFunction|__isInt|__isList|__isPath|__isString|__langVersion|__length|__lessThan|__listToAttrs|__mapAttrs|__match|__mul|__parseDrvName|__partition|__path|__pathExists|__readDir|__readFile|__replaceStrings|__seq|__sort|__split|__splitVersion|__storePath|__stringLength|__sub|__substring|__tail|__toFile|__toJSON|__toPath|__toXML|__trace|__tryEval|__typeOf|__unsafeDiscardOutputDependency|__unsafeDiscardStringContext|__unsafeGetAttrPos|__valueSize|abort|baseNameOf|derivation|derivationStrict|dirOf|fetchGit|fetchMercurial|fetchTarball|fromTOML|import|isNull|map|placeholder|removeAttrs|scopedImport|throw|toString)$"))
