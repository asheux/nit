//! Tree-sitter language detection and parsing for the seed encoders.
//!
//! `SeedLanguage` is duplicated here (rather than reused from `nit-syntax`)
//! to avoid a dependency cycle — `nit-syntax` already depends on
//! `nit-core`. The variant set tracks `nit_syntax::LanguageId`, minus:
//!   - `PlainText` (enum-only sentinel),
//!   - `Dockerfile` (grammar crate wedged at an older tree-sitter ABI),
//!   - `Wolfram` (no tree-sitter crate compatible with 0.25; the
//!     status-bar label is set in `LANGUAGES` but the encoders have no
//!     parser to feed).
//!
//! `Dotenv` rides on the bash grammar — `.env` files are shell-style
//! `KEY=value` assignments, and the bash parser produces a usable AST
//! for them. `is_code: false` in `LANGUAGES` keeps the workspace scan
//! from genome-scoring config files in practice, but the parser arm is
//! wired so a `Buffer` opened from `.env` still gets a tree.
//!
//! Path → variant resolution delegates to [`crate::languages`], the
//! single source of truth for label / extension / filename mapping. Only
//! the `tree_sitter_<lang>::LANGUAGE` references stay here, because those
//! crate dependencies cannot live inside `nit_core::languages`.

use std::path::Path;

use tree_sitter::{Parser, Tree};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum SeedLanguage {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Markdown,
    Html,
    Css,
    Json,
    Toml,
    Yaml,
    Bash,
    Go,
    C,
    Cpp,
    Java,
    Ruby,
    Lua,
    Php,
    OCaml,
    Haskell,
    Elixir,
    Nix,
    Kotlin,
    Sql,
    Zig,
    Make,
    Lean,
    Swift,
    Dotenv,
}

impl SeedLanguage {
    /// Map a `LANGUAGES` label back to a `SeedLanguage` variant. Returns
    /// `None` for labels the seed encoders do not parse: `dockerfile`
    /// (ABI mismatch) and `wolfram` (no compatible grammar).
    pub(crate) fn from_label(label: &str) -> Option<Self> {
        Some(match label {
            "rust" => Self::Rust,
            "python" => Self::Python,
            "javascript" => Self::JavaScript,
            "typescript" => Self::TypeScript,
            "markdown" => Self::Markdown,
            "html" => Self::Html,
            "css" => Self::Css,
            "json" => Self::Json,
            "toml" => Self::Toml,
            "yaml" => Self::Yaml,
            "bash" => Self::Bash,
            "go" => Self::Go,
            "c" => Self::C,
            "cpp" => Self::Cpp,
            "java" => Self::Java,
            "ruby" => Self::Ruby,
            "lua" => Self::Lua,
            "php" => Self::Php,
            "ocaml" => Self::OCaml,
            "haskell" => Self::Haskell,
            "elixir" => Self::Elixir,
            "nix" => Self::Nix,
            "kotlin" => Self::Kotlin,
            "sql" => Self::Sql,
            "zig" => Self::Zig,
            "make" => Self::Make,
            "lean" => Self::Lean,
            "swift" => Self::Swift,
            "dotenv" => Self::Dotenv,
            _ => return None,
        })
    }

    fn detect(file_path: Option<&Path>) -> Option<Self> {
        let info = crate::languages::detect_by_path(file_path?)?;
        Self::from_label(info.label)
    }

    pub(crate) fn ts_language(self) -> tree_sitter::Language {
        // All grammars on tree-sitter 0.25 expose `LANGUAGE: LanguageFn`;
        // lean4 still ships `fn language()`.
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Markdown => tree_sitter_md::LANGUAGE.into(),
            Self::Html => tree_sitter_html::LANGUAGE.into(),
            Self::Css => tree_sitter_css::LANGUAGE.into(),
            Self::Json => tree_sitter_json::LANGUAGE.into(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Self::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Self::Bash => tree_sitter_bash::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::C => tree_sitter_c::LANGUAGE.into(),
            Self::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::Ruby => tree_sitter_ruby::LANGUAGE.into(),
            Self::Lua => tree_sitter_lua::LANGUAGE.into(),
            Self::Php => tree_sitter_php::LANGUAGE_PHP.into(),
            Self::OCaml => tree_sitter_ocaml::LANGUAGE_OCAML.into(),
            Self::Haskell => tree_sitter_haskell::LANGUAGE.into(),
            Self::Elixir => tree_sitter_elixir::LANGUAGE.into(),
            Self::Nix => tree_sitter_nix::LANGUAGE.into(),
            Self::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
            Self::Sql => tree_sitter_sequel::LANGUAGE.into(),
            Self::Zig => tree_sitter_zig::LANGUAGE.into(),
            Self::Make => tree_sitter_make::LANGUAGE.into(),
            Self::Lean => tree_sitter_lean4::language(),
            Self::Swift => tree_sitter_swift::LANGUAGE.into(),
            // Dotenv reuses the bash grammar; see the module-level
            // comment for the rationale.
            Self::Dotenv => tree_sitter_bash::LANGUAGE.into(),
        }
    }
}

pub(crate) fn seed_parse(text: &str, file_path: Option<&Path>) -> Option<(Tree, SeedLanguage)> {
    let lang = SeedLanguage::detect(file_path)?;
    let mut parser = Parser::new();
    parser.set_language(&lang.ts_language()).ok()?;
    let tree = parser.parse(text, None)?;
    Some((tree, lang))
}
