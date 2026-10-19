//! AWK grammar for tree-sitter, vendored from
//! [`Beaglefoot/tree-sitter-awk`](https://github.com/Beaglefoot/tree-sitter-awk)
//! (MIT) at commit `34bbdc7cce8e803096f47b625979e34c1be38127` (tag `v0.7.2`).
//!
//! Upstream is not published to crates.io and its Rust binding pins
//! `tree-sitter ~0.20`, returning that crate's `Language` type, which is
//! incompatible with nit's tree-sitter 0.25. We instead vendor only the C
//! parser + C external scanner (compiled by `build.rs`) and expose the
//! version-agnostic [`LanguageFn`], which the 0.25 runtime accepts via
//! `.into()`. The parser is ABI 14, within the range tree-sitter 0.25 loads.

use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_awk() -> *const ();
}

/// The tree-sitter [`LanguageFn`] for AWK. Convert to a
/// `tree_sitter::Language` with `tree_sitter_awk::LANGUAGE.into()`.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_awk) };

/// The highlight query shipped with the grammar (`queries/highlights.scm`).
/// The nit-specific edits to it are listed in that file's header comment.
pub const HIGHLIGHTS_QUERY: &str = include_str!("../queries/highlights.scm");

/// The grammar's `node-types.json`.
pub const NODE_TYPES: &str = include_str!("node-types.json");

#[cfg(test)]
mod tests {
    #[test]
    fn can_load_grammar() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&super::LANGUAGE.into())
            .expect("loading the AWK grammar should succeed");
    }

    #[test]
    fn parses_a_begin_block() {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&super::LANGUAGE.into()).unwrap();
        // A `BEGIN` pattern, a block, a `print` statement, and a string: the
        // external scanner has to agree with the parser for the block to close.
        let tree = parser
            .parse("BEGIN { print \"hi\" }\n", None)
            .expect("parse should yield a tree");
        assert_eq!(tree.root_node().kind(), "program");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn highlights_query_compiles() {
        tree_sitter::Query::new(&super::LANGUAGE.into(), super::HIGHLIGHTS_QUERY)
            .expect("highlights.scm should compile against the grammar");
    }
}
