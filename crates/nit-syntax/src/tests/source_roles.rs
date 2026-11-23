use super::{make_request, wait_for, TreeSitterEngine};
use crate::{HighlightGroup, LanguageId, SyntaxEngine, ViewportRange};

#[test]
fn every_language_paints_representative_tokens_on_open_edit_and_scroll() {
    use HighlightGroup::*;
    type Sample = (
        &'static str,
        &'static str,
        &'static [(&'static str, HighlightGroup)],
    );
    let samples: &[Sample] = &[
        (
            "rust",
            "use std::path::Path;\nfn review(item: &Path) { Reader::open(item).finish(); }\n",
            &[
                ("review", Function),
                ("Path", Type),
                ("Reader", Type),
                ("open", Function),
                ("finish", Method),
            ],
        ),
        (
            "python",
            "def review(item):\n    return print(\"ready\")\n",
            &[("review", Function), ("return", Keyword), ("ready", String)],
        ),
        (
            "javascript",
            "function review(item) { return item.finish(42); }\n",
            &[("review", Function), ("finish", Method), ("42", Number)],
        ),
        (
            "typescript",
            "function review(item: number) { return item + 42; }\n",
            &[
                ("review", Function),
                ("item", Parameter),
                ("number", TypeBuiltin),
            ],
        ),
        (
            "tsx",
            "function Review() { return <article title=\"ready\" />; }\n",
            &[("article", Tag), ("title", Attribute), ("ready", String)],
        ),
        (
            "markdown",
            "# Review\n\n[home]: /index\n",
            &[("Review", Heading), ("/index", Link)],
        ),
        (
            "html",
            "<article title=\"ready\">Read</article>\n",
            &[("article", Tag), ("title", Attribute), ("ready", String)],
        ),
        (
            "css",
            "article { width: 42px; }\n",
            &[("article", Tag), ("width", Property), ("42", Number)],
        ),
        (
            "json",
            "{ \"ready\": true, \"count\": 42 }\n",
            &[("ready", Property), ("true", Boolean), ("42", Number)],
        ),
        (
            "toml",
            "title = \"ready\"\ncount = 42\n",
            &[("title", Property), ("ready", String), ("42", Number)],
        ),
        (
            "yaml",
            "title: ready\ncount: 42\n",
            &[("title", Property), ("ready", String), ("42", Number)],
        ),
        (
            "bash",
            "review() { printf \"ready\"; }\n",
            &[("review", Function), ("ready", String)],
        ),
        (
            "awk",
            "function review(a) { return a + 42 }\n",
            &[("review", Function), ("42", Number)],
        ),
        (
            "go",
            "package main\nfunc review(item int) int { return item + 42 }\n",
            &[
                ("review", Function),
                ("return", KeywordControl),
                ("42", Number),
            ],
        ),
        (
            "c",
            "int review(int item) { return item + 42; }\n",
            &[("review", Function), ("42", Number)],
        ),
        (
            "cpp",
            "class Reader { int review(int item) { return item + 42; } };\n",
            &[("Reader", Type), ("42", Number)],
        ),
        (
            "java",
            "class Reader { int review(int item) { return item + 42; } }\n",
            &[("Reader", Type), ("review", Method), ("42", Number)],
        ),
        (
            "ruby",
            "def review(item)\n  item + 42\nend\n",
            &[("review", Function), ("42", Number)],
        ),
        (
            "lua",
            "local function review(item) return item + 42 end\n",
            &[("review", Function), ("42", Number)],
        ),
        (
            "php",
            "<?php function review($item) { return $item + 42; }\n",
            &[("review", Function), ("42", Number)],
        ),
        (
            "ocaml",
            "let review item = item + 42\n",
            &[("review", Function), ("42", Number)],
        ),
        (
            "haskell",
            "module Review where\nreview item = item + 42\n",
            &[("review", Function), ("42", Number)],
        ),
        (
            "elixir",
            "defmodule Reader do\n  def review(item), do: item + 42\nend\n",
            &[("review", Function), ("42", Number)],
        ),
        (
            "nix",
            "let count = 42; in { title = \"ready\"; }\n",
            &[("let", Keyword), ("42", Number), ("ready", String)],
        ),
        (
            "kotlin",
            "fun review(item: Int): Int { return item + 42 }\n",
            &[("review", Function), ("42", Number)],
        ),
        (
            "sql",
            "SELECT count(*) FROM records WHERE id = 42;\n",
            &[("SELECT", Keyword), ("42", Number)],
        ),
        (
            "zig",
            "fn review(item: i32) i32 { return item + 42; }\n",
            &[("review", Function), ("42", Number)],
        ),
        (
            "make",
            "include common.mk\nreview: build\n\t@echo ready\n",
            &[("include", Keyword), ("review", Function)],
        ),
        (
            "lean",
            "def review (item : Nat) : Nat := item + 42\n",
            &[("review", Function), ("42", Number)],
        ),
        (
            "swift",
            "func review(item: Int) -> Int { return item + 42 }\n",
            &[("review", Function), ("42", Number)],
        ),
        (
            "dockerfile",
            "FROM alpine:3.20\nLABEL title=\"ready\"\n",
            &[("FROM", Keyword), ("ready", String)],
        ),
        (
            "dotenv",
            "TITLE=\"ready\"\nCOUNT=42\n",
            &[("TITLE", Constant), ("ready", String), ("42", Number)],
        ),
        (
            "wolfram",
            "If[x > 42, Print[x], Null]\n",
            &[("If", KeywordControl), ("42", Number), ("Print", Function)],
        ),
    ];
    let mut engine = TreeSitterEngine::new();
    let mut failures = Vec::new();
    for (id, &(label, source, expected)) in samples.iter().enumerate() {
        let Some(language) = LanguageId::from_label(label) else {
            failures.push(format!("{label}: missing language"));
            continue;
        };
        let end_of_line = source.find('\n').unwrap();
        let mut edited = source.to_string();
        edited.insert(end_of_line, ' ');
        for version in 1..=3 {
            let source = if version == 1 { source } else { &edited };
            let mut request = make_request(id, version, language, source);
            if version == 2 {
                request.full_reparse = false;
                let point = |column| nit_core::BufferPoint { row: 0, column };
                request.edits.push(nit_core::BufferEdit {
                    start_byte: end_of_line,
                    old_end_byte: end_of_line,
                    new_end_byte: end_of_line + 1,
                    start_point: point(end_of_line),
                    old_end_point: point(end_of_line),
                    new_end_point: point(end_of_line + 1),
                });
            } else if version == 3 {
                request.viewport = Some(ViewportRange {
                    first_line: 0,
                    last_line: source.lines().count() - 1,
                    total_lines: source.lines().count(),
                });
            }
            engine.schedule_rehighlight(request);
            let snapshot = wait_for(&mut engine, id, version);
            for &(token, group) in expected {
                let byte = source.find(token).unwrap();
                let line = source[..byte].bytes().filter(|b| *b == b'\n').count();
                let start = byte - snapshot.line_start_bytes[line];
                for col in start..start + token.len() {
                    let actual = snapshot.per_line[line]
                        .iter()
                        .rev()
                        .find(|s| s.start <= col && col < s.end)
                        .map_or(Normal, |s| s.group);
                    if actual != group {
                        failures.push(format!(
                            "{label} pass {version}: {token} expected {group:?}, got {actual:?}"
                        ));
                        break;
                    }
                }
            }
        }
    }
    for language in LanguageId::ALL {
        assert!(
            samples.iter().any(|s| s.0 == language.label()),
            "missing {language} fixture"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
