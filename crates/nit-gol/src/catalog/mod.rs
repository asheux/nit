//! Rule catalog: built-in rules, user overlays, and lookup indices.
//!
//! Loads the bundled `rules.toml` catalog at compile time and optionally
//! merges user-defined overlays from the configuration directory. The
//! catalog provides lookup by id, rulestring, alias, and free-text filter.
//!
//! Overlays are keyed by id (case-insensitive). An overlay whose id
//! matches an existing entry merges field-by-field; an overlay with an
//! unknown id becomes a new user entry, provided its rulestring is valid
//! and does not duplicate a rulestring already in the catalog.

mod helpers;
mod overlay;
mod types;

use std::collections::{HashMap, HashSet};
use std::fs;

use nit_utils::paths;

use helpers::{build_search_haystack, normalize_key, rule_key};
use overlay::{apply_overlays, build_entry_from_file, RuleFile, RuleOverlayFile};

pub use types::{
    RuleDefaultParams, RuleEntry, RuleOverlay, RuleSelectError, RuleSource, SelectedRule,
};

use crate::Rule;

const DEFAULT_RULES_TOML: &str = include_str!("../../assets/rules.toml");

/// Indexed collection of rule entries with lookup by id, rule, and alias.
#[derive(Clone, Debug)]
pub struct RuleCatalog {
    entries: Vec<RuleEntry>,
    visible_indices: Vec<usize>,
    id_index: HashMap<String, usize>,
    rule_index: HashMap<u32, usize>,
    alias_index: HashMap<String, usize>,
}

impl RuleCatalog {
    /// Load the built-in catalog and merge any user overlay file.
    pub fn load() -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut catalog = Self::load_builtin(&mut warnings);
        if let Some(path) = user_overlay_path() {
            match read_overlay_file(&path) {
                Ok(file) => apply_overlays(&mut catalog.entries, &file.rules, &mut warnings),
                Err(err) => warnings.push(format!("Failed to parse rules overlay: {err}")),
            }
        }
        catalog.rebuild_indices(&mut warnings);
        (catalog, warnings)
    }

    /// Load built-ins, then apply additional overlays on top.
    pub fn load_with_overlays(extras: &[RuleOverlay]) -> (Self, Vec<String>) {
        let (mut catalog, mut warnings) = Self::load();
        apply_overlays(&mut catalog.entries, extras, &mut warnings);
        catalog.rebuild_indices(&mut warnings);
        (catalog, warnings)
    }

    /// Number of visible (non-hidden) entries.
    pub fn len(&self) -> usize {
        self.visible_indices.len()
    }

    /// True when the visible catalog has no entries.
    pub fn is_empty(&self) -> bool {
        self.visible_indices.is_empty()
    }

    /// Iterate over non-hidden built-in entries.
    pub fn builtins(&self) -> impl Iterator<Item = &RuleEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.source == RuleSource::Builtin && !entry.hidden)
    }

    /// Iterate over all visible entries in catalog order.
    pub fn iter(&self) -> impl Iterator<Item = &RuleEntry> {
        self.visible_indices
            .iter()
            .filter_map(|idx| self.entries.get(*idx))
    }

    /// Get a visible entry by position index.
    pub fn get(&self, idx: usize) -> Option<&RuleEntry> {
        self.visible_indices
            .get(idx)
            .and_then(|idx| self.entries.get(*idx))
    }

    /// Look up an entry by its canonical id (case-insensitive).
    pub fn find_by_id(&self, id: &str) -> Option<&RuleEntry> {
        self.id_index
            .get(&normalize_key(id))
            .and_then(|idx| self.entries.get(*idx))
    }

    /// Look up an entry by its parsed rule value.
    pub fn find_by_rule(&self, rule: Rule) -> Option<&RuleEntry> {
        self.rule_index
            .get(&rule_key(rule))
            .and_then(|idx| self.entries.get(*idx))
    }

    /// Find the visible-list position of a selected rule.
    pub fn index_of_selected(&self, selected: &SelectedRule) -> Option<usize> {
        let entry_idx = selected
            .id
            .as_deref()
            .and_then(|id| self.id_index.get(&normalize_key(id)).copied())
            .or_else(|| self.rule_index.get(&rule_key(selected.rule)).copied())?;
        self.visible_indices.iter().position(|v| *v == entry_idx)
    }

    /// Return visible-list positions matching a free-text query.
    ///
    /// The query is matched case-insensitively against id, name,
    /// rulestring, description, tags, and aliases. An empty query
    /// returns every visible entry in catalog order.
    pub fn filter_indices(&self, query: &str) -> Vec<usize> {
        let needle = query.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return (0..self.visible_indices.len()).collect();
        }
        self.visible_indices
            .iter()
            .enumerate()
            .filter_map(|(pos, idx)| {
                let entry = self.entries.get(*idx)?;
                build_search_haystack(entry)
                    .to_ascii_lowercase()
                    .contains(&needle)
                    .then_some(pos)
            })
            .collect()
    }

    /// Resolve a user-provided selector string to a [`SelectedRule`].
    ///
    /// Tries, in order: exact id, alias, and finally a raw rulestring
    /// parse. A successful rulestring parse is enriched with catalog
    /// metadata when the rule has a known entry.
    pub fn select(&self, selector: &str) -> Result<SelectedRule, RuleSelectError> {
        let trimmed = selector.trim();
        if trimmed.is_empty() {
            return Err(RuleSelectError::UnknownId(selector.to_string()));
        }
        let key = normalize_key(trimmed);
        let named = self
            .id_index
            .get(&key)
            .or_else(|| self.alias_index.get(&key))
            .and_then(|idx| self.entries.get(*idx));
        if let Some(entry) = named {
            return Ok(SelectedRule::from_named(entry));
        }
        let rule = Rule::parse(trimmed).map_err(RuleSelectError::Parse)?;
        let mut selected = SelectedRule::from_rule(rule);
        if let Some(entry) = self.find_by_rule(rule) {
            selected.id = Some(entry.id.clone());
            selected.name = Some(entry.name.clone());
        }
        Ok(selected)
    }

    /// Format a rule as `rulestring (name)` if it exists in the catalog.
    pub fn label_for_rule(&self, rule: Rule) -> String {
        match self.find_by_rule(rule) {
            Some(named) => format!("{} ({})", rule, named.name),
            None => rule.to_string(),
        }
    }

    fn load_builtin(warnings: &mut Vec<String>) -> Self {
        let file: RuleFile =
            toml::from_str(DEFAULT_RULES_TOML).expect("builtin rules catalog parse");
        let entries = parse_builtin_entries(file);
        let mut catalog = Self::from_entries(entries);
        catalog.rebuild_indices(warnings);
        catalog
    }

    fn rebuild_indices(&mut self, warnings: &mut Vec<String>) {
        self.visible_indices.clear();
        self.id_index.clear();
        self.rule_index.clear();
        self.alias_index.clear();
        for (idx, entry) in self.entries.iter().enumerate() {
            index_entry(
                idx,
                entry,
                warnings,
                &mut self.id_index,
                &mut self.rule_index,
            );
            for alias in &entry.aliases {
                self.alias_index
                    .entry(alias.to_ascii_lowercase())
                    .or_insert(idx);
            }
            if !entry.hidden {
                self.visible_indices.push(idx);
            }
        }
    }

    /// Applies overlays in-place and rebuilds derived indices. Exposed
    /// for tests that mutate catalog state outside the `load` flow.
    #[cfg(test)]
    fn apply_overlays(&mut self, overlays: &[RuleOverlay], warnings: &mut Vec<String>) {
        apply_overlays(&mut self.entries, overlays, warnings);
    }

    fn from_entries(entries: Vec<RuleEntry>) -> Self {
        Self {
            entries,
            visible_indices: Vec::new(),
            id_index: HashMap::new(),
            rule_index: HashMap::new(),
            alias_index: HashMap::new(),
        }
    }
}

impl Default for RuleCatalog {
    fn default() -> Self {
        let mut warnings = Vec::new();
        Self::load_builtin(&mut warnings)
    }
}

fn user_overlay_path() -> Option<std::path::PathBuf> {
    paths::config_dir()
        .map(|dir| dir.join("rules.toml"))
        .filter(|p| p.exists())
}

fn read_overlay_file(path: &std::path::Path) -> Result<RuleOverlayFile, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    toml::from_str::<RuleOverlayFile>(&text).map_err(|e| e.to_string())
}

fn parse_builtin_entries(file: RuleFile) -> Vec<RuleEntry> {
    let mut entries = Vec::new();
    let mut ids = HashSet::new();
    let mut rules = HashSet::new();
    for raw in file.rules {
        let built = match build_entry_from_file(raw, RuleSource::Builtin) {
            Ok(entry) => entry,
            Err(err) => panic!("builtin rule load failed: {err}"),
        };
        if !ids.insert(built.id.to_ascii_lowercase()) {
            panic!("duplicate builtin rule id '{}'", built.id);
        }
        if !rules.insert(rule_key(built.rule)) {
            panic!("duplicate builtin rulestring '{}'", built.rulestring);
        }
        entries.push(built);
    }
    entries
}

fn index_entry(
    idx: usize,
    entry: &RuleEntry,
    warnings: &mut Vec<String>,
    id_index: &mut HashMap<String, usize>,
    rule_index: &mut HashMap<u32, usize>,
) {
    if id_index
        .insert(entry.id.to_ascii_lowercase(), idx)
        .is_some()
    {
        warnings.push(format!("Duplicate rule id '{}'", entry.id));
    }
    if rule_index.insert(rule_key(entry.rule), idx).is_some() {
        warnings.push(format!(
            "Duplicate rulestring for rule '{}': {}",
            entry.id, entry.rulestring
        ));
    }
}

#[cfg(test)]
#[path = "../test_modules/catalog.rs"]
mod tests;
