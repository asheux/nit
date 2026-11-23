//! Grammar-time configuration: compiled queries and highlight-group
//! resolutions for every [`LanguageId`] that ships a grammar.

use std::{collections::HashMap, sync::OnceLock};

use tracing::{debug, warn};
use tree_sitter::Query;
use tree_sitter_highlight::HighlightConfiguration;

use crate::highlight::HighlightGroup;
use crate::language::{LanguageId, LanguageRegistry};

use super::table::{capture_names, CAPTURES};

pub(crate) struct QueryConfig {
    pub query: Query,
    pub highlight_groups: Vec<Option<HighlightGroup>>,
}

impl QueryConfig {
    pub fn highlight_for_index(&self, capture_idx: usize) -> HighlightGroup {
        self.highlight_groups
            .get(capture_idx)
            .copied()
            .flatten()
            .unwrap_or(HighlightGroup::Normal)
    }
}

// Only immutable compiled queries are shared. Parsers, highlighters and query
// cursors stay with the worker that executes them. PlainText has its own empty slot.
const SLOTS: usize = LanguageId::ALL.len() + 1;

pub(crate) fn highlight_config(lang: LanguageId) -> Option<&'static HighlightConfiguration> {
    static CONFIGS: [OnceLock<Option<HighlightConfiguration>>; SLOTS] =
        [const { OnceLock::new() }; SLOTS];
    CONFIGS[lang as usize]
        .get_or_init(|| {
            let grammar = LanguageRegistry::tree_sitter_language(lang)?;
            let highlights = LanguageRegistry::highlights_query(lang)?;
            let injections = LanguageRegistry::injections_query(lang);
            let Some(mut cfg) = try_build_config(grammar, highlights, injections) else {
                warn!("highlight config for {lang:?} failed (with and without injections)");
                return None;
            };
            cfg.configure(&capture_names());
            Some(cfg)
        })
        .as_ref()
}

/// The inline grammar is an internal child of Markdown, never a standalone file language.
pub(crate) fn markdown_inline_config() -> Option<&'static HighlightConfiguration> {
    static INLINE: std::sync::OnceLock<Option<HighlightConfiguration>> = std::sync::OnceLock::new();
    INLINE
        .get_or_init(|| {
            let mut config = try_build_config(
                tree_sitter_md::INLINE_LANGUAGE.into(),
                include_str!("../../queries/markdown/inline.scm"),
                "",
            )?;
            config.configure(&capture_names());
            Some(config)
        })
        .as_ref()
}

// Injection queries can fail to compile for some grammars; retry without them
// so the language still loads with bare highlights.
fn try_build_config(
    grammar: tree_sitter::Language,
    highlights: &str,
    injections: &str,
) -> Option<HighlightConfiguration> {
    // tree-sitter-highlight 0.25 added a `name` parameter for diagnostics;
    // we pass a generic "nit" label since the lang is logged at the
    // call site anyway.
    match HighlightConfiguration::new(grammar.clone(), "nit", highlights, injections, "") {
        Ok(cfg) => Some(cfg),
        Err(err) => {
            debug!("injections failed ({err}), retrying without");
            HighlightConfiguration::new(grammar, "nit", highlights, "", "").ok()
        }
    }
}

pub(crate) fn query_config(lang: LanguageId) -> Option<&'static QueryConfig> {
    static CONFIGS: [OnceLock<Option<QueryConfig>>; SLOTS] = [const { OnceLock::new() }; SLOTS];
    CONFIGS[lang as usize]
        .get_or_init(|| {
            let grammar = LanguageRegistry::tree_sitter_language(lang)?;
            let highlights = LanguageRegistry::highlights_query(lang)?;
            let query = Query::new(&grammar, highlights).ok()?;
            let groups = CAPTURES.iter().copied().collect();
            let highlight_groups = resolve_highlight_groups(&query, &groups);
            Some(QueryConfig {
                query,
                highlight_groups,
            })
        })
        .as_ref()
}

fn resolve_highlight_groups(
    query: &Query,
    groups: &HashMap<&str, HighlightGroup>,
) -> Vec<Option<HighlightGroup>> {
    query
        .capture_names()
        .iter()
        .map(|name| lookup_with_parent_fallback(name, groups))
        .collect()
}

// `"function.method"` falls back to `"function"` when no exact match exists.
fn lookup_with_parent_fallback(
    name: &str,
    groups: &HashMap<&str, HighlightGroup>,
) -> Option<HighlightGroup> {
    if let Some(&group) = groups.get(name) {
        return Some(group);
    }
    let root = name.split('.').next()?;
    groups.get(root).copied()
}
