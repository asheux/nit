use std::path::Path;

use tree_sitter::{Parser, Tree};

use crate::seed::encoders::SeedLanguage;

pub(super) fn ts_parse(text: &str, file_path: &Path) -> Option<Tree> {
    // Resolve grammar through the central languages table so filename-keyed
    // languages (Makefile, Gemfile, Dockerfile) are scanned alongside
    // extension-keyed ones. `SeedLanguage::from_label` returns `None` for
    // `dockerfile` (its grammar crate is wedged at an older ABI), so the
    // scanner gracefully skips files the encoder cannot parse.
    let info = crate::languages::detect_by_path(file_path)?;
    let lang = SeedLanguage::from_label(info.label)?;
    let mut parser = Parser::new();
    parser.set_language(&lang.ts_language()).ok()?;
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
