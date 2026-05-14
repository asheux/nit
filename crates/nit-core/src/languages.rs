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
}

/// The master table. Edit here to add a language; every other gate in
/// the workspace pulls from this list.
///
/// `Dockerfile` is listed so its extension/filename resolution works,
/// but `nit-syntax::grammars::tree_sitter_language` returns `None` for
/// it (upstream `tree-sitter-dockerfile` is wedged at an old ABI).
pub const LANGUAGES: &[LanguageInfo] = &[
    LanguageInfo {
        label: "rust",
        display: "Rust",
        extensions: &["rs"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["rs", "rust"],
        is_code: true,
    },
    LanguageInfo {
        label: "python",
        display: "Python",
        extensions: &["py"],
        filenames: &[],
        shebangs: &["python", "python3"],
        injection_aliases: &["py", "python"],
        is_code: true,
    },
    LanguageInfo {
        label: "javascript",
        display: "JavaScript",
        extensions: &["js", "mjs", "cjs", "jsx"],
        filenames: &[],
        shebangs: &["node", "deno"],
        injection_aliases: &["js", "javascript"],
        is_code: true,
    },
    LanguageInfo {
        label: "typescript",
        display: "TypeScript",
        extensions: &["ts", "tsx"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["ts", "tsx", "typescript"],
        is_code: true,
    },
    LanguageInfo {
        label: "markdown",
        display: "Markdown",
        extensions: &["md", "markdown"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["md", "markdown"],
        is_code: false,
    },
    LanguageInfo {
        label: "html",
        display: "HTML",
        extensions: &["html", "htm"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["html"],
        is_code: true,
    },
    LanguageInfo {
        label: "css",
        display: "CSS",
        extensions: &["css", "scss", "sass"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["css"],
        is_code: true,
    },
    LanguageInfo {
        label: "json",
        display: "JSON",
        extensions: &["json", "jsonc"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["json", "jsonc", "geojson"],
        is_code: false,
    },
    LanguageInfo {
        label: "toml",
        display: "TOML",
        extensions: &["toml"],
        filenames: &["cargo.toml"],
        shebangs: &[],
        injection_aliases: &["toml"],
        is_code: false,
    },
    LanguageInfo {
        label: "yaml",
        display: "YAML",
        extensions: &["yml", "yaml"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["yaml", "yml"],
        is_code: false,
    },
    LanguageInfo {
        label: "bash",
        display: "Bash",
        extensions: &["sh", "bash", "zsh", "fish"],
        filenames: &[],
        shebangs: &["bash", "sh", "zsh"],
        injection_aliases: &["bash", "sh", "shell", "shell-session", "console", "zsh"],
        is_code: true,
    },
    LanguageInfo {
        label: "go",
        display: "Go",
        extensions: &["go"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["go", "golang"],
        is_code: true,
    },
    LanguageInfo {
        label: "c",
        display: "C",
        extensions: &["c", "h"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["c"],
        is_code: true,
    },
    LanguageInfo {
        label: "cpp",
        display: "C++",
        extensions: &["cc", "cpp", "cxx", "c++", "hh", "hpp", "hxx", "h++"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["cpp", "c++", "cxx"],
        is_code: true,
    },
    LanguageInfo {
        label: "java",
        display: "Java",
        extensions: &["java"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["java"],
        is_code: true,
    },
    LanguageInfo {
        label: "ruby",
        display: "Ruby",
        extensions: &["rb", "rake", "gemspec"],
        filenames: &["rakefile", "gemfile"],
        shebangs: &["ruby"],
        injection_aliases: &["rb", "ruby"],
        is_code: true,
    },
    LanguageInfo {
        label: "lua",
        display: "Lua",
        extensions: &["lua"],
        filenames: &[],
        shebangs: &["lua"],
        injection_aliases: &["lua"],
        is_code: true,
    },
    LanguageInfo {
        label: "php",
        display: "PHP",
        extensions: &["php", "phtml"],
        filenames: &[],
        shebangs: &["php"],
        injection_aliases: &["php"],
        is_code: true,
    },
    LanguageInfo {
        label: "ocaml",
        display: "OCaml",
        extensions: &["ml", "mli"],
        filenames: &[],
        shebangs: &["ocaml", "ocamlrun"],
        injection_aliases: &["ocaml", "ml"],
        is_code: true,
    },
    LanguageInfo {
        label: "haskell",
        display: "Haskell",
        extensions: &["hs", "lhs"],
        filenames: &[],
        shebangs: &["runghc", "runhaskell"],
        injection_aliases: &["haskell", "hs"],
        is_code: true,
    },
    LanguageInfo {
        label: "elixir",
        display: "Elixir",
        extensions: &["ex", "exs"],
        filenames: &[],
        shebangs: &["elixir"],
        injection_aliases: &["elixir", "ex"],
        is_code: true,
    },
    LanguageInfo {
        label: "nix",
        display: "Nix",
        extensions: &["nix"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["nix"],
        is_code: true,
    },
    LanguageInfo {
        label: "kotlin",
        display: "Kotlin",
        extensions: &["kt", "kts"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["kotlin", "kt"],
        is_code: true,
    },
    LanguageInfo {
        label: "sql",
        display: "SQL",
        extensions: &["sql"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["sql"],
        is_code: true,
    },
    LanguageInfo {
        label: "zig",
        display: "Zig",
        extensions: &["zig", "zon"],
        filenames: &[],
        shebangs: &[],
        injection_aliases: &["zig"],
        is_code: true,
    },
    LanguageInfo {
        label: "make",
        display: "Makefile",
        extensions: &["mk", "make"],
        filenames: &["makefile", "gnumakefile"],
        shebangs: &[],
        injection_aliases: &["make", "makefile"],
        is_code: false,
    },
    LanguageInfo {
        label: "lean",
        display: "Lean",
        extensions: &["lean"],
        filenames: &[],
        shebangs: &["lean"],
        injection_aliases: &["lean", "lean4"],
        is_code: true,
    },
    LanguageInfo {
        label: "swift",
        display: "Swift",
        extensions: &["swift"],
        filenames: &[],
        shebangs: &["swift"],
        injection_aliases: &["swift"],
        is_code: true,
    },
    LanguageInfo {
        label: "dockerfile",
        display: "Dockerfile",
        extensions: &[],
        filenames: &["dockerfile", "containerfile"],
        shebangs: &[],
        injection_aliases: &["dockerfile", "docker"],
        is_code: false,
    },
];

/// Markup / config / data extensions that the editor will track but
/// that don't get a tree-sitter genome score. Kept here so the
/// file-watcher's "broader than CODE_EXTENSIONS" set has one source.
pub const MARKUP_AUXILIARY_EXTENSIONS: &[&str] = &["txt", "conf"];

// ---------- Lookup helpers ----------

/// Resolve a file's language by trying filename match → extension match →
/// pattern-based filename suffix/prefix (Dockerfile.prod, foo.mk, etc.).
/// Returns `None` if the path doesn't match any registered language.
#[must_use]
pub fn detect_by_path(path: &Path) -> Option<&'static LanguageInfo> {
    if let Some(filename) = path.file_name().and_then(|os| os.to_str()) {
        let lower = filename.to_ascii_lowercase();
        if let Some(info) = detect_by_filename(&lower) {
            return Some(info);
        }
        // Pattern-based filenames: Dockerfile.<x>, <x>.dockerfile, etc.
        // Each language entry lists its primary filenames; suffix/prefix
        // variants are checked here so we don't have to enumerate every
        // possible Dockerfile.* in the table.
        if lower.starts_with("dockerfile.") || lower.ends_with(".dockerfile") {
            return detect_by_label("dockerfile");
        }
        if lower.starts_with("makefile.") || lower.ends_with(".make") || lower.ends_with(".mk") {
            return detect_by_label("make");
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
}
