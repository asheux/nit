//! Rule catalog: built-in rules, user overlays, and lookup indices.
//!
//! Loads the bundled `rules.toml` at compile time and optionally merges
//! user-defined overlays from the configuration directory. Lookup is
//! offered by id, rulestring, alias, and free-text filter.
//!
//! Overlays are keyed by id (case-insensitive). An overlay whose id
//! matches an existing entry merges field-by-field; an overlay with an
//! unknown id becomes a new user entry, provided its rulestring is
//! valid and does not duplicate one already in the catalog.

mod overlay;
mod types;

use std::collections::{HashMap, HashSet};
use std::fs;

use nit_utils::paths;

use overlay::{apply_overlays, build_entry_from_file, RuleFile, RuleOverlayFile};

pub use types::{
    RuleDefaultParams, RuleEntry, RuleOverlay, RuleSelectError, RuleSource, SelectedRule,
};

use crate::Rule;

const DEFAULT_RULES_TOML: &str = include_str!("../../assets/rules.toml");

/// Compact `u32` packing of a rule's birth and survive masks (9 bits
/// each) into one word — `rule_index` is keyed by this packing, so the
/// layout cannot change silently.
pub(super) fn rule_key(rule: Rule) -> u32 {
    (u32::from(rule.births_mask()) << 9) | u32::from(rule.survives_mask())
}

/// Concatenated searchable fields for case-insensitive substring filtering.
fn build_search_haystack(entry: &RuleEntry) -> String {
    let fixed = [
        entry.id.as_str(),
        entry.name.as_str(),
        entry.rulestring.as_str(),
        entry.description.as_str(),
    ];
    let extras = entry
        .tags
        .iter()
        .chain(entry.aliases.iter())
        .map(String::as_str);
    fixed
        .into_iter()
        .chain(extras)
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Debug)]
pub struct RuleCatalog {
    entries: Vec<RuleEntry>,
    visible_indices: Vec<usize>,
    id_index: HashMap<String, usize>,
    rule_index: HashMap<u32, usize>,
    alias_index: HashMap<String, usize>,
}

impl RuleCatalog {
    /// Load the built-in catalog and merge any user overlay file from
    /// the configuration directory (silently skipped if absent).
    pub fn load() -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut catalog = Self::load_builtin(&mut warnings);
        let overlay_path = paths::config_dir()
            .map(|dir| dir.join("rules.toml"))
            .filter(|p| p.exists());
        if let Some(path) = overlay_path {
            let parsed = fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| {
                    toml::from_str::<RuleOverlayFile>(&text).map_err(|e| e.to_string())
                });
            match parsed {
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

    pub fn len(&self) -> usize {
        self.visible_indices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.visible_indices.is_empty()
    }

    /// Iterate over non-hidden built-in entries.
    pub fn builtins(&self) -> impl Iterator<Item = &RuleEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.source == RuleSource::Builtin && !entry.hidden)
    }

    /// Iterate over visible entries in catalog order.
    pub fn iter(&self) -> impl Iterator<Item = &RuleEntry> {
        self.visible_indices
            .iter()
            .filter_map(|idx| self.entries.get(*idx))
    }

    pub fn get(&self, idx: usize) -> Option<&RuleEntry> {
        self.visible_indices
            .get(idx)
            .and_then(|idx| self.entries.get(*idx))
    }

    /// Look up by canonical id (case-insensitive). Aliases are NOT
    /// consulted here — use [`select`](Self::select) for that.
    pub fn find_by_id(&self, id: &str) -> Option<&RuleEntry> {
        let idx = *self.id_index.get(&id.trim().to_ascii_lowercase())?;
        self.entries.get(idx)
    }

    pub fn find_by_rule(&self, rule: Rule) -> Option<&RuleEntry> {
        let idx = *self.rule_index.get(&rule_key(rule))?;
        self.entries.get(idx)
    }

    /// Visible-list position of a selected rule, preferring id over
    /// rulestring match when both are known.
    pub fn index_of_selected(&self, selected: &SelectedRule) -> Option<usize> {
        let entry_idx = selected
            .id
            .as_deref()
            .and_then(|id| self.id_index.get(&id.trim().to_ascii_lowercase()).copied())
            .or_else(|| self.rule_index.get(&rule_key(selected.rule)).copied())?;
        self.visible_indices.iter().position(|v| *v == entry_idx)
    }

    /// Visible-list positions matching a free-text query (id, name,
    /// rulestring, description, tags, aliases — case-insensitive). An
    /// empty query returns every visible entry in catalog order.
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

    /// Resolve a selector string to a [`SelectedRule`]: try id, then
    /// alias, then a raw rulestring parse. A successful rulestring
    /// parse is enriched with catalog metadata when the rule has a
    /// matching entry.
    pub fn select(&self, selector: &str) -> Result<SelectedRule, RuleSelectError> {
        let trimmed = selector.trim();
        if trimmed.is_empty() {
            return Err(RuleSelectError::UnknownId(selector.to_string()));
        }
        let key = trimmed.to_ascii_lowercase();
        let entry_idx = self
            .id_index
            .get(&key)
            .or_else(|| self.alias_index.get(&key))
            .copied();
        if let Some(entry) = entry_idx.and_then(|idx| self.entries.get(idx)) {
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

    /// Format a rule as `rulestring (name)` if it has a catalog entry.
    pub fn label_for_rule(&self, rule: Rule) -> String {
        match self.find_by_rule(rule) {
            Some(named) => format!("{} ({})", rule, named.name),
            None => rule.to_string(),
        }
    }

    fn load_builtin(warnings: &mut Vec<String>) -> Self {
        let file: RuleFile =
            toml::from_str(DEFAULT_RULES_TOML).expect("builtin rules catalog parse");
        let mut entries = Vec::with_capacity(file.rules.len());
        let mut seen_ids: HashSet<String> = HashSet::with_capacity(file.rules.len());
        let mut seen_rules: HashSet<u32> = HashSet::with_capacity(file.rules.len());
        for raw in file.rules {
            let built = build_entry_from_file(raw, RuleSource::Builtin)
                .unwrap_or_else(|err| panic!("builtin rule load failed: {err}"));
            assert!(
                seen_ids.insert(built.id.to_ascii_lowercase()),
                "duplicate builtin rule id '{}'",
                built.id
            );
            assert!(
                seen_rules.insert(rule_key(built.rule)),
                "duplicate builtin rulestring '{}'",
                built.rulestring
            );
            entries.push(built);
        }
        let mut catalog = Self {
            entries,
            visible_indices: Vec::new(),
            id_index: HashMap::new(),
            rule_index: HashMap::new(),
            alias_index: HashMap::new(),
        };
        catalog.rebuild_indices(warnings);
        catalog
    }

    fn rebuild_indices(&mut self, warnings: &mut Vec<String>) {
        self.visible_indices.clear();
        self.id_index.clear();
        self.rule_index.clear();
        self.alias_index.clear();
        for (idx, entry) in self.entries.iter().enumerate() {
            if self
                .id_index
                .insert(entry.id.to_ascii_lowercase(), idx)
                .is_some()
            {
                warnings.push(format!("Duplicate rule id '{}'", entry.id));
            }
            if self.rule_index.insert(rule_key(entry.rule), idx).is_some() {
                warnings.push(format!(
                    "Duplicate rulestring for rule '{}': {}",
                    entry.id, entry.rulestring
                ));
            }
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

    /// Test bridge: apply overlays without rebuilding indices.
    /// Callers must follow with [`rebuild_indices`](Self::rebuild_indices).
    #[cfg(test)]
    fn apply_overlays(&mut self, overlays: &[RuleOverlay], warnings: &mut Vec<String>) {
        apply_overlays(&mut self.entries, overlays, warnings);
    }
}

impl Default for RuleCatalog {
    fn default() -> Self {
        let mut warnings = Vec::new();
        Self::load_builtin(&mut warnings)
    }
}

#[cfg(test)]
#[path = "../test_modules/catalog.rs"]
mod tests;
