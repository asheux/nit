//! Single source of truth for language metadata across the workspace.
//!
//! Adding a new language is **one edit here** plus three grammar-crate
//! references: a Cargo.toml dep, an arm in
//! `nit_syntax::grammars::tree_sitter_language`, and an arm in
//! `nit_core::seed::encoders::lang::SeedLanguage::ts_language`. Every
//! other gate (extension lists, file-watcher filter, swarm scope walk,
//! markdown code-block alias resolver, label diagnostics) derives from
//! the `LANGUAGES` table below.
//!
//! Invariant: each extension belongs to at most one language entry. A
//! debug-build test enforces this so two entries can't silently claim
//! `.rs`.

use std::path::Path;

/// Default indentation policy advertised by a language. Consumed by
/// `Buffer::indent_unit` as the fallback when content sniffing finds no
/// indented lines, and by `insert_tab` so a space-indented language
/// never inserts a stray `\t`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IndentStyle {
    /// Hard tabs (Go, Make).
    Tabs,
    /// Soft tabs of `width` ASCII spaces.
    Spaces(u8),
}

impl IndentStyle {
    /// String suitable for direct insertion into the rope — one tab or
    /// `width` spaces.
    #[must_use]
    pub fn unit(self) -> String {
        match self {
            Self::Tabs => "\t".to_string(),
            Self::Spaces(width) => " ".repeat(width as usize),
        }
    }

    /// `true` for `Tabs`; `false` for any space width.
    #[must_use]
    pub fn uses_tabs(self) -> bool {
        matches!(self, Self::Tabs)
    }

    /// Column width one indent step occupies on screen — `1` for `Tabs`
    /// (one char), `n` for `Spaces(n)`.
    #[must_use]
    pub fn width(self) -> u8 {
        match self {
            Self::Tabs => 1,
            Self::Spaces(n) => n,
        }
    }
}

/// Metadata for one tree-sitter-supported language. Grammar functions
/// live in higher-level crates (they need `tree_sitter_<lang>` crates in
/// scope), so we keep this table grammar-free.
pub struct LanguageInfo {
    /// Stable lowercase identifier used in logs, telemetry, and the
    /// fenced-code-block resolver (`canonical_code_lang`).
    pub label: &'static str,
    /// Human-facing display name shown in the editor's status bar.
    pub display: &'static str,
    /// File extensions (without leading dot, lowercase) that map to this
    /// language. Order matters only for documentation; lookup compares
    /// case-insensitively.
    pub extensions: &'static [&'static str],
    /// Filenames (no extension) that map to this language —
    /// `Makefile`, `Dockerfile`, `Gemfile`, etc. Lowercase match.
    pub filenames: &'static [&'static str],
    /// Interpreter basenames in `#!` shebang lines (`python`, `node`,
    /// `bash`). Empty for compiled / non-scripting languages.
    pub shebangs: &'static [&'static str],
    /// Aliases accepted inside Markdown fenced-code info strings (and
    /// equivalent injection callers). Always includes the canonical
    /// `label`; may add short forms (`rs`, `cpp`).
    pub injection_aliases: &'static [&'static str],
    /// `true` for programming languages worth scoring with the genome
    /// quality pipeline. `false` for markup / data / build files
    /// (markdown, json, toml, yaml, dockerfile, make) — these still get
    /// syntax highlighting + buffer tracking, but the workspace scan
    /// skips them to avoid wasting CPU on files that don't carry the
    /// signal the genome encoders measure.
    pub is_code: bool,
    /// Authoritative indent policy for new files when content sniffing
    /// finds no indented lines. `None` means "let the buffer fall back
    /// to its hard-coded default" (used for data formats where the
    /// editor's default is fine).
    pub default_indent: Option<IndentStyle>,
}

/// The master table. Edit here to add a language; every other gate in
/// the workspace pulls from this list.
///
/// `Dockerfile` is listed so its extension/filename resolution works,
/// but `nit-syntax::grammars::tree_sitter_language` returns `None` for
/// it (upstream `tree-sitter-dockerfile` is wedged at an old ABI).
///
/// `Wolfram` is listed for `.wl` / `.wls` detection; `.m` is not
/// claimed (it overlaps MATLAB and Objective-C, and a wrong default
/// produces visibly broken highlights). No grammar crate currently
/// targets tree-sitter 0.25 cleanly, so the grammar arm returns `None`
/// — the file opens as plain text with a "Wolfram" status label.
///
/// `Dotenv` reuses `tree-sitter-bash` (shell-style `KEY=value`); no
/// dedicated dotenv crate is pulled in. Detection covers `.env`,
/// `.env.local`, `.env.production`, etc. via the `.env*` filename
/// pattern in [`detect_by_path`].
pub const LANGUAGES: &[LanguageInfo] = &[
    LanguageInfo {
        label: "rust",
        display: "Rust",
        extensions: &["rs"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["rs", "rust"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(4)),
    },
    LanguageInfo {
        label: "python",
        display: "Python",
        extensions: &["py"],
        filenames: &[],
        shebangs: &["python", "python3"],
        injection_aliases: &["py", "python"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(4)),
    },
    LanguageInfo {
        label: "javascript",
        display: "JavaScript",
        extensions: &["js", "mjs", "cjs", "jsx"],
        filenames: &[],
        shebangs: &["node", "deno"],
        injection_aliases: &["js", "javascript"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "typescript",
        display: "TypeScript",
        extensions: &["ts", "tsx"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["ts", "tsx", "typescript"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "markdown",
        display: "Markdown",
        extensions: &["md", "markdown"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["md", "markdown"],
        is_code: false,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "html",
        display: "HTML",
        extensions: &["html", "htm"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["html"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "css",
        display: "CSS",
        extensions: &["css", "scss", "sass"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["css"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "json",
        display: "JSON",
        extensions: &["json", "jsonc"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["json", "jsonc", "geojson"],
        is_code: false,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "toml",
        display: "TOML",
        extensions: &["toml"],
        filenames: &["cargo.toml"],
        shebangs: &[],
        injection_aliases: &["toml"],
        is_code: false,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "yaml",
        display: "YAML",
        extensions: &["yml", "yaml"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["yaml", "yml"],
        is_code: false,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "bash",
        display: "Bash",
        extensions: &["sh", "bash", "zsh", "fish"],
        filenames: &[],
        shebangs: &["bash", "sh", "zsh"],
        injection_aliases: &["bash", "sh", "shell", "shell-session", "console", "zsh"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "go",
        display: "Go",
        extensions: &["go"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["go", "golang"],
        is_code: true,
        default_indent: Some(IndentStyle::Tabs),
    },
    LanguageInfo {
        label: "c",
        display: "C",
        extensions: &["c", "h"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["c"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(4)),
    },
    LanguageInfo {
        label: "cpp",
        display: "C++",
        extensions: &["cc", "cpp", "cxx", "c++", "hh", "hpp", "hxx", "h++"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["cpp", "c++", "cxx"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(4)),
    },
    LanguageInfo {
        label: "java",
        display: "Java",
        extensions: &["java"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["java"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(4)),
    },
    LanguageInfo {
        label: "ruby",
        display: "Ruby",
        extensions: &["rb", "rake", "gemspec"],
        filenames: &["rakefile", "gemfile"],
        shebangs: &["ruby"],
        injection_aliases: &["rb", "ruby"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "lua",
        display: "Lua",
        extensions: &["lua"],
        filenames: &[],
        shebangs: &["lua"],
        injection_aliases: &["lua"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "php",
        display: "PHP",
        extensions: &["php", "phtml"],
        filenames: &[],
        shebangs: &["php"],
        injection_aliases: &["php"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(4)),
    },
    LanguageInfo {
        label: "ocaml",
        display: "OCaml",
        extensions: &["ml", "mli"],
        filenames: &[],
        shebangs: &["ocaml", "ocamlrun"],
        injection_aliases: &["ocaml", "ml"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "haskell",
        display: "Haskell",
        extensions: &["hs", "lhs"],
        filenames: &[],
        shebangs: &["runghc", "runhaskell"],
        injection_aliases: &["haskell", "hs"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "elixir",
        display: "Elixir",
        extensions: &["ex", "exs"],
        filenames: &[],
        shebangs: &["elixir"],
        injection_aliases: &["elixir", "ex"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "nix",
        display: "Nix",
        extensions: &["nix"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["nix"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "kotlin",
        display: "Kotlin",
        extensions: &["kt", "kts"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["kotlin", "kt"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(4)),
    },
    LanguageInfo {
        label: "sql",
        display: "SQL",
        extensions: &["sql"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["sql"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "zig",
        display: "Zig",
        extensions: &["zig", "zon"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["zig"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(4)),
    },
    LanguageInfo {
        label: "make",
        display: "Makefile",
        extensions: &["mk", "make"],
        filenames: &["makefile", "gnumakefile"],
        shebangs: &[],
        injection_aliases: &["make", "makefile"],
        is_code: false,
        // GNU Make requires hard tabs for recipe lines; using spaces
        // produces "missing separator" errors.
        default_indent: Some(IndentStyle::Tabs),
    },
    LanguageInfo {
        label: "lean",
        display: "Lean",
        extensions: &["lean"],
        filenames: &[],
        shebangs: &["lean"],
        injection_aliases: &["lean", "lean4"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "swift",
        display: "Swift",
        extensions: &["swift"],
        filenames: &[],
        shebangs: &["swift"],
        injection_aliases: &["swift"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(4)),
    },
    LanguageInfo {
        label: "dockerfile",
        display: "Dockerfile",
        extensions: &[],
        filenames: &["dockerfile", "containerfile"],
        shebangs: &[],
        injection_aliases: &["dockerfile", "docker"],
        is_code: false,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
    LanguageInfo {
        label: "dotenv",
        display: "Dotenv",
        // `.env` filenames are matched by the `.env*` pattern in
        // `detect_by_path`; the bare `.env` is also listed below for
        // direct lookup.
        extensions: &[],
        filenames: &[".env"],
        shebangs: &[],
        injection_aliases: &["dotenv", "env"],
        is_code: false,
        default_indent: None,
    },
    LanguageInfo {
        label: "wolfram",
        display: "Wolfram",
        // `.m` is intentionally omitted — it overlaps MATLAB and
        // Objective-C. Detection sticks to the unambiguous `.wl`
        // (package) and `.wls` (script) extensions; users who want
        // Wolfram highlighting on `.m` files can rename or rely on a
        // future content-sniff hook.
        // TODO(filetype-override-T13): add either (a) a first-non-comment
        // line sniff that picks Wolfram on `Clear[` / `Module[` /
        // `BeginPackage[`, MATLAB on bare `function`, Objective-C on
        // `@interface`, OR (b) a user-facing `:set ft=wolfram` style
        // override that beats path-based detection. Tracked as the
        // T9.1 follow-up to the T9 language-support ticket.
        extensions: &["wl", "wls"],
        filenames: &[],
        shebangs: &["wolframscript"],
        injection_aliases: &["wolfram", "mathematica"],
        is_code: true,
        default_indent: Some(IndentStyle::Spaces(2)),
    },
];

/// Markup / config / data extensions that the editor will track but
/// that don't get a tree-sitter genome score. Kept here so the
/// file-watcher's "broader than CODE_EXTENSIONS" set has one source.
pub const MARKUP_AUXILIARY_EXTENSIONS: &[&str] = &["txt", "conf"];

// ---------- Lookup helpers ----------

/// Resolve a file's language by trying filename match → extension match →
/// pattern-based filename suffix/prefix (Dockerfile.prod, foo.mk,
/// .env.production, etc.). Returns `None` if the path doesn't match any
/// registered language.
#[must_use]
pub fn detect_by_path(path: &Path) -> Option<&'static LanguageInfo> {
    if let Some(filename) = path.file_name().and_then(|os| os.to_str()) {
        let lower = filename.to_ascii_lowercase();
        if let Some(info) = detect_by_filename(&lower) {
            return Some(info);
        }
        // Pattern-based filenames: Dockerfile.<x>, <x>.dockerfile,
        // .env.<x>, etc. Each language entry lists its primary filenames;
        // suffix/prefix variants are checked here so we don't have to
        // enumerate every possible Dockerfile.* / .env.* in the table.
        if lower.starts_with("dockerfile.") || lower.ends_with(".dockerfile") {
            return detect_by_label("dockerfile");
        }
        if lower.starts_with("makefile.") || lower.ends_with(".make") || lower.ends_with(".mk") {
            return detect_by_label("make");
        }
        // `.env`, `.env.local`, `.env.production`, etc. The bare `.env`
        // is already handled by `detect_by_filename` above; this arm
        // catches every dotted variant (`.env.<tag>`).
        if lower.starts_with(".env.") {
            return detect_by_label("dotenv");
        }
    }
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .and_then(|ext| detect_by_extension(&ext))
}

/// Resolve by lowercase extension (no leading dot).
#[must_use]
pub fn detect_by_extension(ext: &str) -> Option<&'static LanguageInfo> {
    LANGUAGES.iter().find(|info| info.extensions.contains(&ext))
}

/// Resolve by exact lowercase filename (no path).
#[must_use]
pub fn detect_by_filename(name: &str) -> Option<&'static LanguageInfo> {
    LANGUAGES.iter().find(|info| info.filenames.contains(&name))
}

/// Resolve by interpreter basename in a shebang line.
#[must_use]
pub fn detect_by_shebang(interpreter: &str) -> Option<&'static LanguageInfo> {
    let lower = interpreter.to_ascii_lowercase();
    let lower_ref: &str = lower.as_str();
    LANGUAGES
        .iter()
        .find(|info| info.shebangs.contains(&lower_ref))
}

/// Resolve by injection alias (Markdown fenced-code language tag, HTML
/// `<script type="...">`, etc.).
#[must_use]
pub fn detect_by_injection_alias(alias: &str) -> Option<&'static LanguageInfo> {
    let lower = alias.to_ascii_lowercase();
    let lower_ref: &str = lower.as_str();
    LANGUAGES
        .iter()
        .find(|info| info.injection_aliases.contains(&lower_ref))
}

/// Resolve by canonical label.
#[must_use]
pub fn detect_by_label(label: &str) -> Option<&'static LanguageInfo> {
    LANGUAGES.iter().find(|info| info.label == label)
}

/// True when any registered language claims this lowercase extension.
#[must_use]
pub fn is_supported_extension(ext: &str) -> bool {
    detect_by_extension(ext).is_some()
}

/// Every extension across every language. Useful for building file-tree
/// filter sets without redeclaring the list per call-site.
pub fn all_extensions() -> impl Iterator<Item = &'static str> {
    LANGUAGES
        .iter()
        .flat_map(|info| info.extensions.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn each_extension_belongs_to_exactly_one_language() {
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for info in LANGUAGES {
            for &ext in info.extensions {
                if let Some(prev) = seen.insert(ext, info.label) {
                    panic!(
                        "extension {ext:?} claimed by both {prev:?} and {:?}",
                        info.label
                    );
                }
            }
        }
    }

    #[test]
    fn each_label_is_unique() {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for info in LANGUAGES {
            assert!(seen.insert(info.label), "duplicate label {:?}", info.label);
        }
    }

    #[test]
    fn injection_aliases_include_canonical_label() {
        for info in LANGUAGES {
            assert!(
                info.injection_aliases.contains(&info.label),
                "{:?}'s injection_aliases must include its canonical label",
                info.label
            );
        }
    }

    #[test]
    fn detect_by_path_handles_dockerfile_variants() {
        for name in [
            "Dockerfile",
            "containerfile",
            "Dockerfile.prod",
            "prod.dockerfile",
        ] {
            let info = detect_by_path(Path::new(name))
                .unwrap_or_else(|| panic!("expected match for {name}"));
            assert_eq!(info.label, "dockerfile", "{name}");
        }
    }

    #[test]
    fn detect_by_path_handles_makefile_variants() {
        for name in [
            "Makefile",
            "GNUmakefile",
            "Makefile.local",
            "build.mk",
            "rules.make",
        ] {
            let info = detect_by_path(Path::new(name))
                .unwrap_or_else(|| panic!("expected match for {name}"));
            assert_eq!(info.label, "make", "{name}");
        }
    }

    #[test]
    fn detect_by_path_handles_dotenv_variants() {
        for name in [
            ".env",
            ".env.local",
            ".env.production",
            ".env.development.local",
        ] {
            let info = detect_by_path(Path::new(name))
                .unwrap_or_else(|| panic!("expected match for {name}"));
            assert_eq!(info.label, "dotenv", "{name}");
        }
    }

    #[test]
    fn detect_by_path_handles_wolfram_extensions() {
        for name in ["foo.wl", "math.wls", "FooBar.WL"] {
            let info = detect_by_path(Path::new(name))
                .unwrap_or_else(|| panic!("expected match for {name}"));
            assert_eq!(info.label, "wolfram", "{name}");
        }
    }

    #[test]
    fn detect_by_path_leaves_dot_m_unclaimed() {
        // `.m` overlaps MATLAB / Objective-C / Mathematica; the safer
        // default is plain text (no entry). See `LANGUAGES`'s Wolfram
        // doc comment for the rationale.
        assert!(detect_by_path(Path::new("foo.m")).is_none());
    }

    #[test]
    fn detect_by_path_does_not_misclaim_env_substring_filenames() {
        // `environment.json` should resolve to JSON via extension, not
        // dotenv via the `.env*` pattern.
        let info = detect_by_path(Path::new("environment.json")).expect("json match");
        assert_eq!(info.label, "json");
    }

    #[test]
    fn default_indent_matrix_matches_language_conventions() {
        // Spot-check the per-language indent defaults. Drives the T10
        // fallback consumed by `Buffer::indent_unit` and `insert_tab`.
        let cases = [
            ("rust", Some(IndentStyle::Spaces(4))),
            ("python", Some(IndentStyle::Spaces(4))),
            ("go", Some(IndentStyle::Tabs)),
            ("make", Some(IndentStyle::Tabs)),
            ("javascript", Some(IndentStyle::Spaces(2))),
            ("typescript", Some(IndentStyle::Spaces(2))),
            ("html", Some(IndentStyle::Spaces(2))),
            ("css", Some(IndentStyle::Spaces(2))),
            ("markdown", Some(IndentStyle::Spaces(2))),
            ("bash", Some(IndentStyle::Spaces(2))),
            ("dotenv", None),
        ];
        for (label, expected) in cases {
            let info = detect_by_label(label)
                .unwrap_or_else(|| panic!("missing language entry for {label}"));
            assert_eq!(info.default_indent, expected, "{label}");
        }
    }

    #[test]
    fn indent_style_unit_matches_width() {
        assert_eq!(IndentStyle::Tabs.unit(), "\t");
        assert_eq!(IndentStyle::Spaces(2).unit(), "  ");
        assert_eq!(IndentStyle::Spaces(4).unit(), "    ");
        assert!(IndentStyle::Tabs.uses_tabs());
        assert!(!IndentStyle::Spaces(4).uses_tabs());
        assert_eq!(IndentStyle::Spaces(4).width(), 4);
        assert_eq!(IndentStyle::Tabs.width(), 1);
    }
}
