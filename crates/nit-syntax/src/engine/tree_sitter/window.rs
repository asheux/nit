//! Highlight one viewport or background-fill chunk. Embedded languages need
//! their enclosing delimiter even when the visible window starts inside it.
use std::collections::HashMap;

use tree_sitter::{QueryCursor, Tree};
use tree_sitter_highlight::{HighlightConfiguration, Highlighter};

use super::modes::{collect_spans, highlight_source};
use crate::captures::QueryConfig;
use crate::highlight::{find_line, HighlightSpan};
use crate::language::{LanguageId, LanguageRegistry};

pub(super) struct WindowHighlighter<'a> {
    pub configs: &'a HashMap<LanguageId, HighlightConfiguration>,
    pub queries: &'a HashMap<LanguageId, QueryConfig>,
    pub highlighter: &'a mut Highlighter,
}

impl WindowHighlighter<'_> {
    pub fn spans(
        &mut self,
        language: LanguageId,
        tree: &Tree,
        text: &str,
        offsets: &[usize],
        lines: (usize, usize),
        cursor: &mut QueryCursor,
    ) -> anyhow::Result<(Vec<HighlightSpan>, usize, usize)> {
        let (first, last) = lines;
        let start = offsets[first];
        let end = offsets.get(last + 1).copied().unwrap_or(text.len());
        if start >= end || LanguageRegistry::injections_query(language).is_empty() {
            let mut spans = Vec::new();
            let query = self
                .queries
                .get(&language)
                .ok_or_else(|| anyhow::anyhow!("no query config"))?;
            collect_spans(
                query,
                tree,
                text.as_bytes(),
                &[(start, end)],
                &mut spans,
                cursor,
            );
            return Ok((spans, first, last));
        }

        // Expand to whole host constructs at each edge. A viewport deep inside
        // a script or fence then parses with its actual language and lexical
        // context. Publishing the expanded rows also skips them in later fills.
        let first = first.min(find_line(offsets, enclosing_range(tree, start).start));
        let last_byte = enclosing_range(tree, end - 1).end.saturating_sub(1);
        let last = last
            .max(find_line(offsets, last_byte))
            .min(offsets.len() - 2);
        let start = offsets[first];
        let end = offsets[last + 1];
        let config = self
            .configs
            .get(&language)
            .ok_or_else(|| anyhow::anyhow!("no highlight config"))?;
        let mut spans = highlight_source(
            self.configs,
            config,
            self.highlighter,
            &text.as_bytes()[start..end],
        )?;
        for span in &mut spans {
            span.start_byte += start;
            span.end_byte += start;
        }
        Ok((spans, first, last))
    }
}

fn enclosing_range(tree: &Tree, byte: usize) -> std::ops::Range<usize> {
    let mut range = byte..byte + 1;
    let mut node = tree.root_node().descendant_for_byte_range(byte, byte + 1);
    while let Some(at) = node {
        match at.kind() {
            "script_element"
            | "style_element"
            | "start_tag"
            | "end_tag"
            | "comment"
            | "paragraph"
            | "fenced_code_block"
            | "indented_code_block"
            | "block_quote"
            | "atx_heading"
            | "setext_heading"
            | "pipe_table" => range = at.byte_range(),
            _ => {}
        }
        node = at.parent();
    }
    range
}

#[cfg(test)]
#[path = "../../tests/embeddings.rs"]
mod tests;
