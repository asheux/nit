//! Wolfram Language grammar for tree-sitter, vendored from
//! [`bostick/tree-sitter-wolfram`](https://github.com/bostick/tree-sitter-wolfram) (MIT).
//!
//! Upstream is not published to crates.io and its Rust binding pins
//! `tree-sitter ~0.20`, returning that crate's `Language` type — incompatible
//! with nit's tree-sitter 0.25. We instead vendor only the C parser + C++
//! external scanner (compiled by `build.rs`) and expose the version-agnostic
//! [`LanguageFn`], which the 0.25 runtime accepts via `.into()`. The parser is
//! ABI 13, still within the range tree-sitter 0.25 can load.

use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_wolfram() -> *const ();
}

/// The tree-sitter [`LanguageFn`] for Wolfram Language. Convert to a
/// `tree_sitter::Language` with `tree_sitter_wolfram::LANGUAGE.into()`.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_wolfram) };

#[cfg(test)]
mod tests {
    #[test]
    fn can_load_grammar() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&super::LANGUAGE.into())
            .expect("loading the Wolfram grammar should succeed");
    }

    #[test]
    fn parses_a_basic_expression() {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&super::LANGUAGE.into()).unwrap();
        // Symbols, `=`, arithmetic, integers, a call, and the external comment
        // scanner via `(* *)` — all node types this grammar supports.
        let tree = parser
            .parse("(* demo *)\nx = 1 + 2 * Sin[3]\n", None)
            .expect("parse should yield a tree");
        assert_eq!(tree.root_node().kind(), "source_file");
        assert!(!tree.root_node().has_error());
    }
}
