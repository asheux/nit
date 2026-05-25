use std::path::Path;

use nit_core::languages::{detect_by_path, LANGUAGES};

use crate::{LanguageId, LanguageRegistry};

#[test]
fn shebang_with_arguments_resolves_interpreter() {
    // Bug #1 regression: the pre-fix detect_shebang took the LAST
    // whitespace token, so flags or arg files (e.g. `-tt`, `-i a.py`)
    // were treated as the interpreter and the shebang silently fell
    // through to PlainText. Each input here reproduced the bug; all
    // must now resolve correctly.
    let cases: &[(&str, LanguageId)] = &[
        ("#!/usr/bin/python3 -tt", LanguageId::Python),
        ("#!/usr/bin/env python3 -i a.py", LanguageId::Python),
        ("#!/bin/bash -e", LanguageId::Bash),
        ("#!/usr/bin/env -S deno run", LanguageId::JavaScript),
    ];
    for &(line, expected) in cases {
        let detected = LanguageRegistry::detect(None, Some(line), None);
        assert_eq!(detected, expected, "shebang {line:?}");
    }
}

#[test]
fn shebang_basic_forms_still_work() {
    // Inputs that worked before Bug #1 must still work — guards
    // against an over-eager rewrite of the parser.
    let cases: &[(&str, LanguageId)] = &[
        ("#!/bin/sh", LanguageId::Bash),
        ("#!/usr/bin/python", LanguageId::Python),
        ("#!/usr/bin/env python3", LanguageId::Python),
        ("#!/usr/bin/env node", LanguageId::JavaScript),
    ];
    for &(line, expected) in cases {
        let detected = LanguageRegistry::detect(None, Some(line), None);
        assert_eq!(detected, expected, "shebang {line:?}");
    }
}

#[test]
fn shebang_unknown_interpreter_falls_through_to_plain() {
    let detected = LanguageRegistry::detect(None, Some("#!/usr/bin/perl"), None);
    assert_eq!(detected, LanguageId::PlainText);
}

#[test]
fn detect_explicit_override_beats_shebang() {
    let detected = LanguageRegistry::detect(
        Some(Path::new("script.py")),
        Some("#!/usr/bin/python3"),
        Some(LanguageId::Rust),
    );
    assert_eq!(detected, LanguageId::Rust);
}

#[test]
fn detect_shebang_beats_path_extension() {
    // For files saved with a misleading extension but a real shebang
    // (e.g. `script.txt` containing `#!/usr/bin/python3`), the
    // interpreter line wins over the path.
    let detected = LanguageRegistry::detect(
        Some(Path::new("script.txt")),
        Some("#!/usr/bin/python3"),
        None,
    );
    assert_eq!(detected, LanguageId::Python);
}

#[test]
fn detect_path_extension_lookup() {
    let cases: &[(&str, LanguageId)] = &[
        ("foo.rs", LanguageId::Rust),
        ("foo.py", LanguageId::Python),
        ("foo.tsx", LanguageId::TypeScript),
        ("foo.mjs", LanguageId::JavaScript),
        ("Cargo.toml", LanguageId::Toml),
        ("Makefile", LanguageId::Make),
        ("readme.md", LanguageId::Markdown),
        ("style.scss", LanguageId::Css),
        ("config.yaml", LanguageId::Yaml),
    ];
    for &(name, expected) in cases {
        let detected = LanguageRegistry::detect(Some(Path::new(name)), None, None);
        assert_eq!(detected, expected, "path {name:?}");
    }
}

#[test]
fn detect_unknown_path_returns_plain() {
    let detected = LanguageRegistry::detect(Some(Path::new("data.bin")), None, None);
    assert_eq!(detected, LanguageId::PlainText);
}

#[test]
fn detect_expanded_path_extensions() {
    // Regression test for the language expansion (Go, C/C++, Java, Ruby,
    // Lua, PHP, OCaml, Haskell, Elixir, Swift, Dockerfile).
    let cases: &[(&str, LanguageId)] = &[
        ("server.go", LanguageId::Go),
        ("hello.c", LanguageId::C),
        ("hello.h", LanguageId::C),
        ("widget.cpp", LanguageId::Cpp),
        ("widget.hpp", LanguageId::Cpp),
        ("Main.java", LanguageId::Java),
        ("app.rb", LanguageId::Ruby),
        ("Gemfile", LanguageId::Ruby),
        ("Rakefile", LanguageId::Ruby),
        ("init.lua", LanguageId::Lua),
        ("index.php", LanguageId::Php),
        ("parser.ml", LanguageId::OCaml),
        ("parser.mli", LanguageId::OCaml),
        ("Main.hs", LanguageId::Haskell),
        ("mod.ex", LanguageId::Elixir),
        ("test.exs", LanguageId::Elixir),
        ("View.swift", LanguageId::Swift),
        ("Dockerfile", LanguageId::Dockerfile),
        ("Containerfile", LanguageId::Dockerfile),
        ("Dockerfile.prod", LanguageId::Dockerfile),
        ("prod.dockerfile", LanguageId::Dockerfile),
    ];
    for &(name, expected) in cases {
        let detected = LanguageRegistry::detect(Some(Path::new(name)), None, None);
        assert_eq!(detected, expected, "path {name:?}");
    }
}

#[test]
fn detect_expanded_shebangs() {
    let cases: &[(&str, LanguageId)] = &[
        ("#!/usr/bin/ruby", LanguageId::Ruby),
        ("#!/usr/bin/env ruby", LanguageId::Ruby),
        ("#!/usr/bin/lua", LanguageId::Lua),
        ("#!/usr/bin/env php", LanguageId::Php),
        ("#!/usr/bin/env ocaml", LanguageId::OCaml),
        ("#!/usr/bin/env runghc", LanguageId::Haskell),
        ("#!/usr/bin/env elixir", LanguageId::Elixir),
        ("#!/usr/bin/swift", LanguageId::Swift),
    ];
    for &(line, expected) in cases {
        let detected = LanguageRegistry::detect(None, Some(line), None);
        assert_eq!(detected, expected, "shebang {line:?}");
    }
}

// Cross-table parity guard: every extension, filename, and injection alias
// listed in `nit_core::languages::LANGUAGES` must resolve under the syntax
// crate's `LanguageRegistry`. Catches drift the moment a language is added
// to the master table without the matching `LanguageId` variant + arms.
#[test]
fn central_table_paths_resolve_through_registry() {
    for info in LANGUAGES {
        for ext in info.extensions {
            let probe = format!("sample.{ext}");
            let detected = LanguageRegistry::detect(Some(Path::new(&probe)), None, None);
            assert_ne!(
                detected,
                LanguageId::PlainText,
                "extension .{ext} from {:?} resolved to PlainText via LanguageRegistry",
                info.label
            );
        }
        for name in info.filenames {
            let detected = LanguageRegistry::detect(Some(Path::new(name)), None, None);
            assert_ne!(
                detected,
                LanguageId::PlainText,
                "filename {name:?} from {:?} resolved to PlainText via LanguageRegistry",
                info.label
            );
        }
    }
}

#[test]
fn central_table_injection_aliases_resolve_through_registry() {
    for info in LANGUAGES {
        // Dockerfile has no shipped tree-sitter language, so its alias
        // legitimately maps to a variant that ignores the alias path.
        if info.label == "dockerfile" {
            continue;
        }
        for alias in info.injection_aliases {
            assert!(
                LanguageRegistry::from_injection_name(alias).is_some(),
                "alias {alias:?} from {:?} returned None via LanguageRegistry",
                info.label
            );
        }
    }
}

#[test]
fn central_table_agrees_with_registry_on_labels() {
    // Both detectors must pick the same language for a given path. We compare
    // by canonical label because the syntax crate's `LanguageId` is a closed
    // enum and `detect_by_path` returns the master table's metadata.
    for info in LANGUAGES {
        for ext in info.extensions {
            let probe = format!("sample.{ext}");
            let core = detect_by_path(Path::new(&probe))
                .map(|i| i.label)
                .unwrap_or("plaintext");
            let registry = LanguageRegistry::detect(Some(Path::new(&probe)), None, None);
            let registry_label = registry_label(registry);
            assert_eq!(
                core, registry_label,
                "drift on .{ext}: core={core:?}, registry={registry_label:?}",
            );
        }
    }
}

fn registry_label(language: LanguageId) -> &'static str {
    match language {
        LanguageId::Rust => "rust",
        LanguageId::Python => "python",
        LanguageId::JavaScript => "javascript",
        LanguageId::TypeScript => "typescript",
        LanguageId::Markdown => "markdown",
        LanguageId::Html => "html",
        LanguageId::Css => "css",
        LanguageId::Json => "json",
        LanguageId::Toml => "toml",
        LanguageId::Yaml => "yaml",
        LanguageId::Bash => "bash",
        LanguageId::Go => "go",
        LanguageId::C => "c",
        LanguageId::Cpp => "cpp",
        LanguageId::Java => "java",
        LanguageId::Ruby => "ruby",
        LanguageId::Lua => "lua",
        LanguageId::Php => "php",
        LanguageId::OCaml => "ocaml",
        LanguageId::Haskell => "haskell",
        LanguageId::Elixir => "elixir",
        LanguageId::Nix => "nix",
        LanguageId::Kotlin => "kotlin",
        LanguageId::Sql => "sql",
        LanguageId::Zig => "zig",
        LanguageId::Make => "make",
        LanguageId::Lean => "lean",
        LanguageId::Swift => "swift",
        LanguageId::Dockerfile => "dockerfile",
        LanguageId::Dotenv => "dotenv",
        LanguageId::Wolfram => "wolfram",
        LanguageId::PlainText => "plaintext",
    }
}
