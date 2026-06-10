//! Per-grammar dispatch: tree-sitter `Language` handles, highlight
//! queries, and injection queries keyed by [`LanguageId`].
//!
//! These three functions are the ONLY per-language tables that must stay
//! distinct from `nit_core::languages::LANGUAGES`: each arm references a
//! different `tree_sitter_<lang>` crate (or an `include_str!` path) that
//! can only be named at compile time. The variant set MUST mirror the
//! `LANGUAGES` table; adding a language is `LANGUAGES` entry + grammar
//! crate dep + one arm each in `tree_sitter_language` and
//! `highlights_query`.

use super::id::LanguageId;

#[must_use]
pub(crate) fn tree_sitter_language(language_id: LanguageId) -> Option<tree_sitter::Language> {
    match language_id {
        LanguageId::Rust => Some(tree_sitter_rust::LANGUAGE.into()),
        LanguageId::Python => Some(tree_sitter_python::LANGUAGE.into()),
        LanguageId::JavaScript => Some(tree_sitter_javascript::LANGUAGE.into()),
        LanguageId::TypeScript => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        LanguageId::Markdown => Some(tree_sitter_md::LANGUAGE.into()),
        LanguageId::Html => Some(tree_sitter_html::LANGUAGE.into()),
        LanguageId::Css => Some(tree_sitter_css::LANGUAGE.into()),
        LanguageId::Json => Some(tree_sitter_json::LANGUAGE.into()),
        LanguageId::Toml => Some(tree_sitter_toml_ng::LANGUAGE.into()),
        LanguageId::Yaml => Some(tree_sitter_yaml::LANGUAGE.into()),
        LanguageId::Bash => Some(tree_sitter_bash::LANGUAGE.into()),
        LanguageId::Go => Some(tree_sitter_go::LANGUAGE.into()),
        LanguageId::C => Some(tree_sitter_c::LANGUAGE.into()),
        LanguageId::Cpp => Some(tree_sitter_cpp::LANGUAGE.into()),
        LanguageId::Java => Some(tree_sitter_java::LANGUAGE.into()),
        LanguageId::Ruby => Some(tree_sitter_ruby::LANGUAGE.into()),
        LanguageId::Lua => Some(tree_sitter_lua::LANGUAGE.into()),
        LanguageId::Php => Some(tree_sitter_php::LANGUAGE_PHP.into()),
        LanguageId::OCaml => Some(tree_sitter_ocaml::LANGUAGE_OCAML.into()),
        LanguageId::Haskell => Some(tree_sitter_haskell::LANGUAGE.into()),
        LanguageId::Elixir => Some(tree_sitter_elixir::LANGUAGE.into()),
        LanguageId::Nix => Some(tree_sitter_nix::LANGUAGE.into()),
        LanguageId::Kotlin => Some(tree_sitter_kotlin_ng::LANGUAGE.into()),
        LanguageId::Sql => Some(tree_sitter_sequel::LANGUAGE.into()),
        LanguageId::Zig => Some(tree_sitter_zig::LANGUAGE.into()),
        LanguageId::Make => Some(tree_sitter_make::LANGUAGE.into()),
        LanguageId::Lean => Some(tree_sitter_lean4::language()),
        LanguageId::Swift => Some(tree_sitter_swift::LANGUAGE.into()),
        // tree-sitter-dockerfile 0.2 is pinned to an older tree-sitter ABI
        // and won't link with our 0.25 runtime. `LANGUAGES` still lists
        // Dockerfile so detection (filename + extension) keeps working;
        // highlights are skipped until upstream ships a compatible crate.
        LanguageId::Dockerfile => None,
        // Dotenv (`.env`, `.env.local`, …) reuses the bash grammar: the
        // file content is shell-style `KEY=value`, and tree-sitter-bash
        // is already a workspace dep — no new crate, no Cargo.lock churn.
        LanguageId::Dotenv => Some(tree_sitter_bash::LANGUAGE.into()),
        // Wolfram has no maintained tree-sitter crate compatible with 0.25.
        // Mirror the Dockerfile arm: `LANGUAGES` claims `.wl` / `.wls` for
        // status-bar labelling and code-block alias resolution, but
        // highlights stay disabled until a usable grammar ships.
        LanguageId::Wolfram => None,
        LanguageId::PlainText => None,
    }
}

#[must_use]
pub(crate) fn highlights_query(language_id: LanguageId) -> Option<&'static str> {
    // Crates that ship `HIGHLIGHTS_QUERY` (sometimes `HIGHLIGHT_QUERY`)
    // expose it directly; everything else uses a hand-rolled query under
    // `queries/<lang>/highlights.scm`.
    match language_id {
        LanguageId::Rust => Some(include_str!("../../queries/rust/highlights.scm")),
        LanguageId::Python => Some(tree_sitter_python::HIGHLIGHTS_QUERY),
        LanguageId::JavaScript => Some(tree_sitter_javascript::HIGHLIGHT_QUERY),
        LanguageId::TypeScript => Some(include_str!("../../queries/typescript/highlights.scm")),
        LanguageId::Markdown => Some(include_str!("../../queries/markdown/highlights.scm")),
        LanguageId::Html => Some(tree_sitter_html::HIGHLIGHTS_QUERY),
        LanguageId::Css => Some(tree_sitter_css::HIGHLIGHTS_QUERY),
        LanguageId::Json => Some(include_str!("../../queries/json/highlights.scm")),
        LanguageId::Toml => Some(tree_sitter_toml_ng::HIGHLIGHTS_QUERY),
        LanguageId::Yaml => Some(include_str!("../../queries/yaml/highlights.scm")),
        LanguageId::Bash => Some(tree_sitter_bash::HIGHLIGHT_QUERY),
        LanguageId::Go => Some(include_str!("../../queries/go/highlights.scm")),
        LanguageId::C => Some(include_str!("../../queries/c/highlights.scm")),
        LanguageId::Cpp => Some(include_str!("../../queries/cpp/highlights.scm")),
        LanguageId::Java => Some(include_str!("../../queries/java/highlights.scm")),
        LanguageId::Ruby => Some(include_str!("../../queries/ruby/highlights.scm")),
        LanguageId::Lua => Some(include_str!("../../queries/lua/highlights.scm")),
        LanguageId::Php => Some(include_str!("../../queries/php/highlights.scm")),
        LanguageId::OCaml => Some(include_str!("../../queries/ocaml/highlights.scm")),
        LanguageId::Haskell => Some(include_str!("../../queries/haskell/highlights.scm")),
        LanguageId::Elixir => Some(include_str!("../../queries/elixir/highlights.scm")),
        LanguageId::Nix => Some(include_str!("../../queries/nix/highlights.scm")),
        LanguageId::Kotlin => Some(include_str!("../../queries/kotlin/highlights.scm")),
        LanguageId::Sql => Some(include_str!("../../queries/sql/highlights.scm")),
        LanguageId::Zig => Some(include_str!("../../queries/zig/highlights.scm")),
        LanguageId::Make => Some(include_str!("../../queries/make/highlights.scm")),
        LanguageId::Lean => Some(include_str!("../../queries/lean/highlights.scm")),
        LanguageId::Swift => Some(include_str!("../../queries/swift/highlights.scm")),
        LanguageId::Dockerfile => Some(include_str!("../../queries/dockerfile/highlights.scm")),
        // The bash grammar's stock highlight query labels every `KEY=value`
        // assignment as the noisy variable kind; a tighter query keeps
        // names crisp.
        LanguageId::Dotenv => Some(include_str!("../../queries/dotenv/highlights.scm")),
        LanguageId::Wolfram => None,
        LanguageId::PlainText => None,
    }
}

#[must_use]
pub(crate) fn injections_query(language_id: LanguageId) -> &'static str {
    // Only host languages that embed code blocks ship an injection query.
    // Everything else returns the empty query so the tree-sitter injector
    // becomes a no-op.
    match language_id {
        LanguageId::Markdown => include_str!("../../queries/markdown/injections.scm"),
        LanguageId::Html => include_str!("../../queries/html/injections.scm"),
        _ => "",
    }
}
