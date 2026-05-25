//! `LanguageId` enum, ordered grammar list, and the bridge between
//! variants and the canonical labels in `nit_core::languages::LANGUAGES`.
//!
//! The enum must stay in lockstep with the central table: every
//! `LanguageInfo` there owns a matching variant here (except `PlainText`,
//! which is the catch-all and intentionally absent from `LANGUAGES`).

use std::fmt;

use nit_core::languages;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum LanguageId {
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
    Dockerfile,
    Dotenv,
    Wolfram,
    PlainText,
}

/// `PlainText`'s synthetic label. Not present in `LANGUAGES` (it's the
/// "no match" sentinel), so it lives here as a private constant.
const PLAIN_TEXT_LABEL: &str = "plaintext";

/// Variant ↔ `LanguageInfo.label` map. Edit this table when adding a
/// new variant; `label`/`from_label`/`Display` all derive from it.
const LABELS: &[(LanguageId, &str)] = &[
    (LanguageId::Rust, "rust"),
    (LanguageId::Python, "python"),
    (LanguageId::JavaScript, "javascript"),
    (LanguageId::TypeScript, "typescript"),
    (LanguageId::Markdown, "markdown"),
    (LanguageId::Html, "html"),
    (LanguageId::Css, "css"),
    (LanguageId::Json, "json"),
    (LanguageId::Toml, "toml"),
    (LanguageId::Yaml, "yaml"),
    (LanguageId::Bash, "bash"),
    (LanguageId::Go, "go"),
    (LanguageId::C, "c"),
    (LanguageId::Cpp, "cpp"),
    (LanguageId::Java, "java"),
    (LanguageId::Ruby, "ruby"),
    (LanguageId::Lua, "lua"),
    (LanguageId::Php, "php"),
    (LanguageId::OCaml, "ocaml"),
    (LanguageId::Haskell, "haskell"),
    (LanguageId::Elixir, "elixir"),
    (LanguageId::Nix, "nix"),
    (LanguageId::Kotlin, "kotlin"),
    (LanguageId::Sql, "sql"),
    (LanguageId::Zig, "zig"),
    (LanguageId::Make, "make"),
    (LanguageId::Lean, "lean"),
    (LanguageId::Swift, "swift"),
    (LanguageId::Dockerfile, "dockerfile"),
    (LanguageId::Dotenv, "dotenv"),
    (LanguageId::Wolfram, "wolfram"),
];

impl LanguageId {
    /// Every variant that ships a grammar slot (excludes [`Self::PlainText`]).
    /// Used by `captures::config` to build per-language highlight configs.
    pub const ALL: [LanguageId; 31] = [
        Self::Rust,
        Self::Python,
        Self::JavaScript,
        Self::TypeScript,
        Self::Markdown,
        Self::Html,
        Self::Css,
        Self::Json,
        Self::Toml,
        Self::Yaml,
        Self::Bash,
        Self::Go,
        Self::C,
        Self::Cpp,
        Self::Java,
        Self::Ruby,
        Self::Lua,
        Self::Php,
        Self::OCaml,
        Self::Haskell,
        Self::Elixir,
        Self::Nix,
        Self::Kotlin,
        Self::Sql,
        Self::Zig,
        Self::Make,
        Self::Lean,
        Self::Swift,
        Self::Dockerfile,
        Self::Dotenv,
        Self::Wolfram,
    ];

    /// Canonical lowercase label, matching `LanguageInfo.label`. `PlainText`
    /// returns a synthetic `"plaintext"` since it has no `LANGUAGES` entry.
    #[must_use]
    pub fn label(self) -> &'static str {
        if matches!(self, Self::PlainText) {
            return PLAIN_TEXT_LABEL;
        }
        LABELS
            .iter()
            .find_map(|&(id, label)| (id == self).then_some(label))
            .unwrap_or(PLAIN_TEXT_LABEL)
    }

    /// Inverse of [`Self::label`]. Bridges a `LanguageInfo` resolved from
    /// `nit_core::languages` back into the variant the rest of the crate
    /// pattern-matches on. Unknown labels (including the empty string)
    /// return `None`; pass `"plaintext"` to get [`Self::PlainText`].
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        if label == PLAIN_TEXT_LABEL {
            return Some(Self::PlainText);
        }
        LABELS
            .iter()
            .find_map(|&(id, l)| (l == label).then_some(id))
    }
}

impl fmt::Display for LanguageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // PlainText has no LANGUAGES entry, so it's handled inline. Every
        // other variant pulls its human-facing name from the central table.
        let display = match self {
            Self::PlainText => "Plain Text",
            other => languages::detect_by_label(other.label())
                .map(|info| info.display)
                .unwrap_or("Plain Text"),
        };
        f.write_str(display)
    }
}
