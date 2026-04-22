//! Grammar-time configuration: compiled queries and highlight-group
//! resolutions for every [`LanguageId`] that ships a grammar.

use std::collections::HashMap;

use tracing::debug;
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

fn grammar_entries() -> impl Iterator<Item = (LanguageId, tree_sitter::Language, &'static str)> {
    LanguageId::ALL.into_iter().filter_map(|lang| {
        let grammar = LanguageRegistry::tree_sitter_language(lang)?;
        let highlights = LanguageRegistry::highlights_query(lang)?;
        Some((lang, grammar, highlights))
    })
}

pub(crate) fn build_highlight_configs() -> HashMap<LanguageId, HighlightConfiguration> {
    let names = capture_names();
    let mut configs = HashMap::with_capacity(LanguageId::ALL.len());

    for (lang, grammar, highlights) in grammar_entries() {
        let injections = LanguageRegistry::injections_query(lang);
        let Some(mut cfg) = try_build_config(grammar, highlights, injections) else {
            debug!("highlight config for {lang:?} failed (with and without injections)");
            continue;
        };
        cfg.configure(&names);
        configs.insert(lang, cfg);
    }

    configs
}

// Injection queries can fail to compile for some grammars; retry without them
// so the language still loads with bare highlights.
fn try_build_config(
    grammar: tree_sitter::Language,
    highlights: &str,
    injections: &str,
) -> Option<HighlightConfiguration> {
    match HighlightConfiguration::new(grammar, highlights, injections, "") {
        Ok(cfg) => Some(cfg),
        Err(err) => {
            debug!("injections failed ({err}), retrying without");
            HighlightConfiguration::new(grammar, highlights, "", "").ok()
        }
    }
}

pub(crate) fn build_query_configs() -> HashMap<LanguageId, QueryConfig> {
    let groups: HashMap<&str, HighlightGroup> = CAPTURES.iter().copied().collect();
    let mut configs = HashMap::with_capacity(LanguageId::ALL.len());

    for (lang, grammar, highlights) in grammar_entries() {
        let Ok(query) = Query::new(grammar, highlights) else {
            continue;
        };
        let highlight_groups = resolve_highlight_groups(&query, &groups);
        configs.insert(
            lang,
            QueryConfig {
                query,
                highlight_groups,
            },
        );
    }

    configs
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
