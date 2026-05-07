use std::path::Path;

use tree_sitter::{Parser, Tree};

pub(super) fn ts_parse(text: &str, file_path: &Path) -> Option<Tree> {
    let ext = file_path.extension()?.to_str()?;
    let language = match ext {
        "rs" => tree_sitter_rust::language(),
        "py" => tree_sitter_python::language(),
        "js" | "jsx" | "mjs" | "cjs" => tree_sitter_javascript::language(),
        "ts" | "tsx" => tree_sitter_typescript::language_typescript(),
        "html" | "htm" => tree_sitter_html::language(),
        "css" => tree_sitter_css::language(),
        "json" => tree_sitter_json::language(),
        "toml" => tree_sitter_toml::language(),
        "sh" | "bash" => tree_sitter_bash::language(),
        _ => return None,
    };
    let mut parser = Parser::new();
    parser.set_language(language).ok()?;
    parser.parse(text, None)
}

/// True for a trimmed line that is syntactically a comment or comment
/// continuation. The `*` rules are narrow on purpose: a bare
/// `starts_with('*')` would misclassify real code like `*ptr = 5` or
/// `*mut T = ...` as a comment, which both undercounts code lines AND
/// inflates the comment ratio enough to wrongly flag a file as
/// comment-padded. Block-comment continuation lines in practice are
/// `* text`, a bare `*`, or `*/`.
pub(super) fn is_comment_line(t: &str) -> bool {
    t.starts_with("//")
        || t.starts_with("/*")
        || t == "*"
        || t.starts_with("* ")
        || t.starts_with("*/")
}
