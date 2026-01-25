use std::path::Path;

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
    PlainText,
}

impl LanguageId {
    pub const ALL: [LanguageId; 11] = [
        LanguageId::Rust,
        LanguageId::Python,
        LanguageId::JavaScript,
        LanguageId::TypeScript,
        LanguageId::Markdown,
        LanguageId::Html,
        LanguageId::Css,
        LanguageId::Json,
        LanguageId::Toml,
        LanguageId::Yaml,
        LanguageId::Bash,
    ];
}

pub struct LanguageRegistry;

impl LanguageRegistry {
    pub fn detect(
        path: Option<&Path>,
        first_line: Option<&str>,
        override_lang: Option<LanguageId>,
    ) -> LanguageId {
        if let Some(lang) = override_lang {
            return lang;
        }
        if let Some(line) = first_line {
            if let Some(lang) = detect_shebang(line) {
                return lang;
            }
        }
        if let Some(path) = path {
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                let lower = name.to_lowercase();
                if lower == "cargo.toml" {
                    return LanguageId::Toml;
                }
                if lower == "makefile" {
                    return LanguageId::Bash;
                }
            }
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                let ext = ext.to_lowercase();
                return match ext.as_str() {
                    "rs" => LanguageId::Rust,
                    "py" => LanguageId::Python,
                    "js" | "mjs" | "cjs" | "jsx" => LanguageId::JavaScript,
                    "ts" | "tsx" => LanguageId::TypeScript,
                    "md" | "markdown" => LanguageId::Markdown,
                    "html" | "htm" => LanguageId::Html,
                    "css" | "scss" | "sass" => LanguageId::Css,
                    "json" | "jsonc" => LanguageId::Json,
                    "toml" => LanguageId::Toml,
                    "yml" | "yaml" => LanguageId::Yaml,
                    "sh" | "bash" | "zsh" | "fish" => LanguageId::Bash,
                    _ => LanguageId::PlainText,
                };
            }
        }
        LanguageId::PlainText
    }

    pub fn tree_sitter_language(id: LanguageId) -> Option<tree_sitter::Language> {
        match id {
            LanguageId::Rust => Some(tree_sitter_rust::language()),
            LanguageId::Python => Some(tree_sitter_python::language()),
            LanguageId::JavaScript => Some(tree_sitter_javascript::language()),
            LanguageId::TypeScript => Some(tree_sitter_typescript::language_typescript()),
            LanguageId::Markdown => Some(tree_sitter_markdown_fork::language()),
            LanguageId::Html => Some(tree_sitter_html::language()),
            LanguageId::Css => Some(tree_sitter_css::language()),
            LanguageId::Json => Some(tree_sitter_json::language()),
            LanguageId::Toml => Some(tree_sitter_toml::language()),
            LanguageId::Yaml => Some(tree_sitter_yaml::language()),
            LanguageId::Bash => Some(tree_sitter_bash::language()),
            LanguageId::PlainText => None,
        }
    }

    pub fn highlights_query(id: LanguageId) -> Option<&'static str> {
        match id {
            LanguageId::Rust => Some(tree_sitter_rust::HIGHLIGHT_QUERY),
            LanguageId::Python => Some(tree_sitter_python::HIGHLIGHT_QUERY),
            LanguageId::JavaScript => Some(tree_sitter_javascript::HIGHLIGHT_QUERY),
            LanguageId::TypeScript => Some(tree_sitter_typescript::HIGHLIGHT_QUERY),
            LanguageId::Markdown => Some(include_str!("../queries/markdown/highlights.scm")),
            LanguageId::Html => Some(tree_sitter_html::HIGHLIGHT_QUERY),
            LanguageId::Css => Some(tree_sitter_css::HIGHLIGHTS_QUERY),
            LanguageId::Json => Some(tree_sitter_json::HIGHLIGHT_QUERY),
            LanguageId::Toml => Some(tree_sitter_toml::HIGHLIGHT_QUERY),
            LanguageId::Yaml => Some(include_str!("../queries/yaml/highlights.scm")),
            LanguageId::Bash => Some(tree_sitter_bash::HIGHLIGHT_QUERY),
            LanguageId::PlainText => None,
        }
    }

    pub fn injections_query(id: LanguageId) -> &'static str {
        match id {
            LanguageId::Markdown => include_str!("../queries/markdown/injections.scm"),
            LanguageId::Html => include_str!("../queries/html/injections.scm"),
            _ => "",
        }
    }

    pub fn from_injection_name(name: &str) -> Option<LanguageId> {
        let lower = name
            .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
            .next()
            .unwrap_or(name)
            .to_lowercase();
        match lower.as_str() {
            "rust" => Some(LanguageId::Rust),
            "python" => Some(LanguageId::Python),
            "javascript" | "js" => Some(LanguageId::JavaScript),
            "typescript" | "ts" | "tsx" => Some(LanguageId::TypeScript),
            "markdown" | "md" => Some(LanguageId::Markdown),
            "html" => Some(LanguageId::Html),
            "css" => Some(LanguageId::Css),
            "json" => Some(LanguageId::Json),
            "toml" => Some(LanguageId::Toml),
            "yaml" | "yml" => Some(LanguageId::Yaml),
            "bash" | "sh" => Some(LanguageId::Bash),
            _ => None,
        }
    }
}

fn detect_shebang(line: &str) -> Option<LanguageId> {
    let line = line.trim();
    if !line.starts_with("#!") {
        return None;
    }
    let lower = line.to_lowercase();
    if lower.contains("python") {
        return Some(LanguageId::Python);
    }
    if lower.contains("node") || lower.contains("deno") {
        return Some(LanguageId::JavaScript);
    }
    if lower.contains("bash") || lower.contains("sh") || lower.contains("zsh") {
        return Some(LanguageId::Bash);
    }
    None
}
