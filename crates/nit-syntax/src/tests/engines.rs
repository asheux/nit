use crate::engine::tree_sitter::TreeSitterEngine;
use crate::{HighlightGroup, LanguageId, SyntaxEngine};

use super::{has_group, make_request, wait_for};

// One parameterized smoke test replaces the per-language keyword assertions:
// every grammar in the table should produce at least one Keyword span on the
// opening line of a minimal program.
#[test]
fn highlights_keywords_per_language() {
    let cases: &[(usize, LanguageId, &str)] = &[
        (1, LanguageId::Rust, "fn main() { let x = 42; }\n"),
        (2, LanguageId::Python, "def foo(x):\n    return x\n"),
        (4, LanguageId::JavaScript, "function foo() { return 1; }\n"),
        // The upstream tree-sitter-typescript HIGHLIGHTS_QUERY only tags
        // TS-specific keywords (`interface`, `type`, `enum`, etc.); JS
        // keywords inherited from the parent grammar are not tagged here.
        (5, LanguageId::TypeScript, "interface Foo {}\n"),
        // Expanded language set — each grammar must produce at least one
        // Keyword on the opening line. Also serves as a guard that the
        // hand-rolled highlights.scm queries parse against their grammars
        // (tree-sitter rejects invalid queries at construction time).
        (100, LanguageId::Go, "package main\n\nfunc main() {}\n"),
        (101, LanguageId::C, "int main(void) { return 0; }\n"),
        (102, LanguageId::Cpp, "class Foo { public: int x; };\n"),
        (
            103,
            LanguageId::Java,
            "public class Foo { void bar() {} }\n",
        ),
        (104, LanguageId::Ruby, "def foo\n  42\nend\n"),
        (105, LanguageId::Lua, "local x = 1\nfunction f() end\n"),
        (106, LanguageId::Php, "<?php function foo() { return 1; }\n"),
        (107, LanguageId::OCaml, "let x = 1\nlet f x = x + 1\n"),
        (108, LanguageId::Haskell, "module M where\nf x = x\n"),
        (
            109,
            LanguageId::Elixir,
            "defmodule M do\n  def f(x), do: x\nend\n",
        ),
        (110, LanguageId::Swift, "func foo() -> Int { return 1 }\n"),
        // Dockerfile crate is wedged at an old tree-sitter ABI (see
        // grammars.rs); reinstate once upstream ships a 0.25-compatible
        // release.
        (112, LanguageId::Nix, "let x = 1; in x + 1\n"),
        (113, LanguageId::Kotlin, "fun main() { val x = 1 }\n"),
        (
            114,
            LanguageId::Zig,
            "const std = @import(\"std\");\nfn main() void {}\n",
        ),
        (115, LanguageId::Lean, "def hello : String := \"world\"\n"),
        (116, LanguageId::Make, "include common.mk\n"),
        (117, LanguageId::Sql, "SELECT 1;\n"),
    ];

    for &(buffer_id, lang, src) in cases {
        let mut engine = TreeSitterEngine::new();
        engine.schedule_rehighlight(make_request(buffer_id, 1, lang, src));
        let snap = wait_for(&mut engine, buffer_id, 1);
        assert!(
            has_group(&snap, 0, HighlightGroup::Keyword)
                || has_group(&snap, 0, HighlightGroup::KeywordControl),
            "{lang:?} did not highlight a Keyword/KeywordControl on line 0"
        );
    }
}

#[test]
fn rust_highlights_numbers() {
    let mut engine = TreeSitterEngine::new();
    let req = make_request(1, 1, LanguageId::Rust, "fn main() { let x = 42; }\n");
    engine.schedule_rehighlight(req);
    let snap = wait_for(&mut engine, 1, 1);
    assert!(has_group(&snap, 0, HighlightGroup::Number));
}

#[test]
fn markdown_highlights_heading() {
    let mut engine = TreeSitterEngine::new();
    let req = make_request(3, 1, LanguageId::Markdown, "# Title\n\nText\n");
    engine.schedule_rehighlight(req);
    let snap = wait_for(&mut engine, 3, 1);
    assert!(has_group(&snap, 0, HighlightGroup::Heading));
}

#[test]
fn json_object_keys_highlight_as_property() {
    let mut engine = TreeSitterEngine::new();
    let req = make_request(
        7,
        1,
        LanguageId::Json,
        "{\n  \"name\": \"x\",\n  \"n\": 1\n}\n",
    );
    engine.schedule_rehighlight(req);
    let snap = wait_for(&mut engine, 7, 1);
    // On line 1 (`"name": "x",`) the key is a Property and the value a String.
    assert!(
        has_group(&snap, 1, HighlightGroup::Property),
        "JSON object key should highlight as Property, not String"
    );
    assert!(has_group(&snap, 1, HighlightGroup::String));
}

#[test]
fn language_change_invalidates_cache() {
    let mut engine = TreeSitterEngine::new();
    let text = "fn main() {}\n";

    engine.schedule_rehighlight(make_request(40, 1, LanguageId::Rust, text));
    let snap1 = wait_for(&mut engine, 40, 1);
    assert_eq!(snap1.language, LanguageId::Rust);

    engine.schedule_rehighlight(make_request(40, 2, LanguageId::Python, text));
    let snap2 = wait_for(&mut engine, 40, 2);
    assert_eq!(snap2.language, LanguageId::Python);
}

#[test]
fn worker_handles_plaintext_then_real_language() {
    let mut engine = TreeSitterEngine::new();

    engine.schedule_rehighlight(make_request(50, 1, LanguageId::PlainText, "hello\n"));
    let snap = wait_for(&mut engine, 50, 1);
    assert_eq!(snap.buffer_id, 50);

    engine.schedule_rehighlight(make_request(51, 1, LanguageId::Rust, "let x = 1;\n"));
    let snap = wait_for(&mut engine, 51, 1);
    assert!(has_group(&snap, 0, HighlightGroup::Keyword));
}

#[test]
fn highlighted_range_none_for_eager_mode() {
    let mut engine = TreeSitterEngine::new();
    let req = make_request(60, 1, LanguageId::Rust, "fn main() {}\n");
    engine.schedule_rehighlight(req);
    let snap = wait_for(&mut engine, 60, 1);
    assert!(
        snap.highlighted_range.is_none(),
        "eager mode should have highlighted_range = None"
    );
}

// Permanent guard: every shipped highlight query must compile against its
// grammar. A single invalid node name or anonymous token makes Query::new
// fail, which silently disables ALL highlighting for that language (0 spans).
// Asserts here with the concrete tree-sitter error so breakage is caught at
// CI time, not by a user noticing a dead language.
#[test]
fn every_highlight_query_compiles() {
    let mut failures = Vec::new();
    for lang in LanguageId::ALL {
        let Some(grammar) = crate::language::LanguageRegistry::tree_sitter_language(lang) else {
            continue;
        };
        let Some(query) = crate::language::LanguageRegistry::highlights_query(lang) else {
            continue;
        };
        if let Err(e) = tree_sitter::Query::new(&grammar, query) {
            failures.push(format!("{lang:?}: {e:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "highlight queries failed to compile:\n{}",
        failures.join("\n")
    );
}

// Regression guard for the full-taxonomy query expansion: every language
// whose hand-rolled query we expanded must capture Operator AND Punctuation
// spans (the two categories that were universally missing before). The
// snippets below all contain operators and brackets, so a query that drops
// back to a comment/string/keyword-only stub fails here.
#[test]
fn expanded_queries_capture_operators_and_punctuation() {
    let cases: &[(LanguageId, &str)] = &[
        (
            LanguageId::Rust,
            "// c\nuse std::fmt;\nfn add(a: i32) -> i32 { let b = a + 1; return b; }\n",
        ),
        (
            LanguageId::Go,
            "package main\nimport \"fmt\"\nfunc add(a int) int { return a + 1 }\n",
        ),
        (
            LanguageId::C,
            "#include <stdio.h>\nint add(int a) { int b = a + 1; return b; }\n",
        ),
        (
            LanguageId::Cpp,
            "class Foo { public: int add(int a) { return a + 1; } };\n",
        ),
        (
            LanguageId::Java,
            "class Foo { int add(int a) { int b = a + 1; return b; } }\n",
        ),
        (
            LanguageId::Python,
            "# c\nimport os\ndef add(a: int) -> int:\n    return a + 1\n",
        ),
        (
            LanguageId::JavaScript,
            "// c\nfunction add(a) { const b = a + 1; return b; }\n",
        ),
        (
            LanguageId::TypeScript,
            "// c\nfunction add(a: number): number { return a + 1; }\n",
        ),
        (
            LanguageId::Ruby,
            "# c\ndef add(a)\n  b = a + 1\n  return b\nend\n",
        ),
        (
            LanguageId::Lua,
            "-- c\nlocal function add(a)\n  local b = a + 1\n  return b\nend\n",
        ),
        (
            LanguageId::Php,
            "<?php\nfunction add($a) { $b = $a + 1; return $b; }\n",
        ),
        (
            LanguageId::OCaml,
            "(* c *)\nlet add a = let b = a + 1 in b\n",
        ),
        (
            LanguageId::Haskell,
            "-- c\nadd :: Int -> Int\nadd a = a + 1\n",
        ),
        (
            LanguageId::Swift,
            "// c\nfunc add(a: Int) -> Int { let b = a + 1; return b }\n",
        ),
        (
            LanguageId::Json,
            "{\n  \"name\": \"x\",\n  \"n\": 42,\n  \"ok\": true\n}\n",
        ),
        (
            LanguageId::Yaml,
            "# c\nname: build\non:\n  push:\n    branches: [main]\n",
        ),
        (LanguageId::Toml, "# c\n[pkg]\nname = \"x\"\nver = 1\n"),
        (
            LanguageId::Sql,
            "-- c\nSELECT id, name FROM users WHERE id = 1;\n",
        ),
        (
            LanguageId::Bash,
            "# c\nset -e\nfor f in *.rs; do echo \"$f\"; done\n",
        ),
        (
            LanguageId::Markdown,
            "# Title\n\nSome **bold** and `code` and [link](url).\n\n- item\n",
        ),
        (
            LanguageId::Css,
            "/* c */\n.foo { color: #fff; width: 10px; }\n",
        ),
        (LanguageId::Html, "<!-- c -->\n<div class=\"x\">hi</div>\n"),
        (LanguageId::Nix, "# c\nlet x = 1; in { y = x + 1; }\n"),
        (
            LanguageId::Kotlin,
            "fun add(a: Int): Int { val b = a + 1; return b }\n",
        ),
        (
            LanguageId::Zig,
            "const std = @import(\"std\");\nfn add(a: i32) i32 { return a + 1; }\n",
        ),
        (
            LanguageId::Elixir,
            "defmodule M do\n  def add(a), do: a + 1\nend\n",
        ),
        (LanguageId::Lean, "-- c\ndef add (a : Nat) : Nat := a + 1\n"),
        (LanguageId::Make, "all: foo\n\tgcc -o foo foo.c\n"),
        (LanguageId::Dotenv, "# c\nKEY=value\nNUM=42\nFLAG=true\n"),
    ];
    // Only the languages whose hand-rolled queries we expanded are asserted
    // here; upstream-query languages (TS/JSON/etc.) and injection-dependent
    // ones (Markdown) are covered by `every_highlight_query_compiles`.
    let must_be_rich = [
        LanguageId::Rust,
        LanguageId::Go,
        LanguageId::TypeScript,
        LanguageId::C,
        LanguageId::Cpp,
        LanguageId::Java,
        LanguageId::Ruby,
        LanguageId::Lua,
        LanguageId::Php,
        LanguageId::Swift,
        LanguageId::Nix,
        LanguageId::Kotlin,
        LanguageId::Zig,
        LanguageId::Sql,
        LanguageId::Lean,
    ];
    for &(lang, src) in cases {
        if !must_be_rich.contains(&lang) {
            continue;
        }
        let mut engine = TreeSitterEngine::new();
        engine.schedule_rehighlight(make_request(900, 1, lang, src));
        let snap = wait_for(&mut engine, 900, 1);
        let has = |g| snap.per_line.iter().flatten().any(|s| s.group == g);
        assert!(
            has(HighlightGroup::Operator),
            "{lang:?} captured no Operator span"
        );
        assert!(
            has(HighlightGroup::Punctuation),
            "{lang:?} captured no Punctuation span"
        );
    }
}
