use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use crate::{Rule, RuleParseError};
use nit_utils::paths;

const DEFAULT_RULES_TOML: &str = include_str!("../assets/rules.toml");

#[derive(Clone, Debug, serde::Deserialize)]
struct RuleFile {
    #[serde(default)]
    rules: Vec<RuleFileEntry>,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct RuleFileEntry {
    id: String,
    #[serde(alias = "name")]
    display_name: String,
    #[serde(alias = "rule")]
    rulestring: String,
    description: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    default_params: RuleDefaultParams,
    #[serde(default)]
    provenance: Vec<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct RuleOverlayFile {
    #[serde(default)]
    rules: Vec<RuleOverlay>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct RuleDefaultParams {
    #[serde(default)]
    pub warning: Option<String>,
    #[serde(default)]
    pub wrap: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct RuleOverlay {
    pub id: String,
    #[serde(default, alias = "name")]
    pub display_name: Option<String>,
    #[serde(default, alias = "rule")]
    pub rulestring: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub aliases: Option<Vec<String>>,
    #[serde(default)]
    pub default_params: Option<RuleDefaultParams>,
    #[serde(default)]
    pub provenance: Option<Vec<String>>,
    #[serde(default)]
    pub favorite: Option<bool>,
    #[serde(default)]
    pub hidden: Option<bool>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RuleSource {
    Builtin,
    User,
}

#[derive(Clone, Debug)]
pub struct RuleEntry {
    pub id: String,
    pub name: String,
    pub rule: Rule,
    pub rulestring: String,
    pub rulestring_raw: String,
    pub description: String,
    pub tags: Vec<String>,
    pub aliases: Vec<String>,
    pub default_params: RuleDefaultParams,
    pub provenance: Vec<String>,
    pub favorite: bool,
    pub hidden: bool,
    pub source: RuleSource,
}

impl RuleEntry {
    pub fn warning(&self) -> Option<&str> {
        self.default_params.warning.as_deref()
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SelectedRule {
    pub rule: Rule,
    pub id: Option<String>,
    pub name: Option<String>,
}

impl SelectedRule {
    pub fn from_rule(rule: Rule) -> Self {
        Self {
            rule,
            id: None,
            name: None,
        }
    }

    pub fn from_named(named: &RuleEntry) -> Self {
        Self {
            rule: named.rule,
            id: Some(named.id.clone()),
            name: Some(named.name.clone()),
        }
    }

    pub fn selector(&self) -> String {
        self.id.clone().unwrap_or_else(|| self.rule.to_string())
    }

    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => format!("{} ({})", self.rule, name),
            None => self.rule.to_string(),
        }
    }

    pub fn name_first_label(&self) -> String {
        match &self.name {
            Some(name) => format!("{} ({})", name, self.rule),
            None => self.rule.to_string(),
        }
    }
}

impl Default for SelectedRule {
    fn default() -> Self {
        SelectedRule::from_rule(Rule::conway())
    }
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
    pub fn load() -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut catalog = Self::load_builtin(&mut warnings);
        if let Some(path) = default_overlay_path().filter(|p| p.exists()) {
            match read_overlay_file(&path) {
                Ok(overlays) => catalog.apply_overlays(&overlays, &mut warnings),
                Err(err) => warnings.push(format!("Failed to parse rules overlay: {err}")),
            }
        }
        catalog.rebuild_indices(&mut warnings);
        (catalog, warnings)
    }

    pub fn load_with_overlays(overlays: &[RuleOverlay]) -> (Self, Vec<String>) {
        let (mut catalog, mut warnings) = Self::load();
        catalog.apply_overlays(overlays, &mut warnings);
        catalog.rebuild_indices(&mut warnings);
        (catalog, warnings)
    }

    pub fn len(&self) -> usize {
        self.visible_indices.len()
    }

    pub fn builtins(&self) -> impl Iterator<Item = &RuleEntry> {
        self.entries
            .iter()
            .filter(|rule| rule.source == RuleSource::Builtin && !rule.hidden)
    }

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

    pub fn find_by_id(&self, id: &str) -> Option<&RuleEntry> {
        let key = id.trim().to_ascii_lowercase();
        self.id_index
            .get(&key)
            .and_then(|idx| self.entries.get(*idx))
    }

    pub fn find_by_rule(&self, rule: Rule) -> Option<&RuleEntry> {
        let key = rule_key(rule);
        self.rule_index
            .get(&key)
            .and_then(|idx| self.entries.get(*idx))
    }

    pub fn index_of_selected(&self, selected: &SelectedRule) -> Option<usize> {
        if let Some(id) = &selected.id {
            let key = id.to_ascii_lowercase();
            if let Some(&idx) = self.id_index.get(&key) {
                return self
                    .visible_indices
                    .iter()
                    .position(|visible| *visible == idx);
            }
        }
        let key = rule_key(selected.rule);
        self.rule_index.get(&key).and_then(|idx| {
            self.visible_indices
                .iter()
                .position(|visible| *visible == *idx)
        })
    }

    pub fn filter_indices(&self, query: &str) -> Vec<usize> {
        let needle = query.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return (0..self.visible_indices.len()).collect();
        }
        self.visible_indices
            .iter()
            .enumerate()
            .filter_map(|(pos, idx)| {
                let Some(rule) = self.entries.get(*idx) else {
                    return None;
                };
                let mut hay = String::new();
                hay.push_str(&rule.id);
                hay.push(' ');
                hay.push_str(&rule.name);
                hay.push(' ');
                hay.push_str(&rule.rulestring);
                hay.push(' ');
                hay.push_str(&rule.description);
                for tag in &rule.tags {
                    hay.push(' ');
                    hay.push_str(tag);
                }
                for alias in &rule.aliases {
                    hay.push(' ');
                    hay.push_str(alias);
                }
                if hay.to_ascii_lowercase().contains(&needle) {
                    Some(pos)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn select(&self, selector: &str) -> Result<SelectedRule, RuleSelectError> {
        let trimmed = selector.trim();
        if trimmed.is_empty() {
            return Err(RuleSelectError::UnknownId(selector.to_string()));
        }
        if let Some(named) = self.find_by_id(trimmed) {
            return Ok(SelectedRule::from_named(named));
        }
        if let Some(named) = self.find_by_alias(trimmed) {
            return Ok(SelectedRule::from_named(named));
        }
        let rule = Rule::parse(trimmed).map_err(RuleSelectError::Parse)?;
        let mut selected = SelectedRule::from_rule(rule);
        if let Some(named) = self.find_by_rule(rule) {
            selected.id = Some(named.id.clone());
            selected.name = Some(named.name.clone());
        }
        Ok(selected)
    }

    pub fn label_for_rule(&self, rule: Rule) -> String {
        match self.find_by_rule(rule) {
            Some(named) => format!("{} ({})", rule, named.name),
            None => rule.to_string(),
        }
    }

    fn find_by_alias(&self, alias: &str) -> Option<&RuleEntry> {
        let key = alias.trim().to_ascii_lowercase();
        self.alias_index
            .get(&key)
            .and_then(|idx| self.entries.get(*idx))
    }

    fn load_builtin(warnings: &mut Vec<String>) -> Self {
        let file: RuleFile =
            toml::from_str(DEFAULT_RULES_TOML).expect("builtin rules catalog parse");
        let mut entries = Vec::new();
        let mut ids = HashSet::new();
        let mut rules = HashSet::new();
        for entry in file.rules {
            let built = match build_entry_from_file(entry, RuleSource::Builtin) {
                Ok(rule) => rule,
                Err(err) => panic!("builtin rule load failed: {err}"),
            };
            let id_key = built.id.to_ascii_lowercase();
            if !ids.insert(id_key) {
                panic!("duplicate builtin rule id '{}'", built.id);
            }
            let key = rule_key(built.rule);
            if !rules.insert(key) {
                panic!("duplicate builtin rulestring '{}'", built.rulestring);
            }
            entries.push(built);
        }
        let mut catalog = Self::from_entries(entries);
        catalog.rebuild_indices(warnings);
        catalog
    }

    fn apply_overlays(&mut self, overlays: &[RuleOverlay], warnings: &mut Vec<String>) {
        let mut id_map = HashMap::new();
        let mut rule_map = HashMap::new();
        for (idx, entry) in self.entries.iter().enumerate() {
            id_map.insert(entry.id.to_ascii_lowercase(), idx);
            rule_map.insert(rule_key(entry.rule), idx);
        }
        for overlay in overlays {
            let id_key = overlay.id.to_ascii_lowercase();
            if let Some(&idx) = id_map.get(&id_key) {
                let entry = &mut self.entries[idx];
                if let Some(rulestring) = overlay.rulestring.as_deref() {
                    match normalize_rulestring(rulestring) {
                        Ok((rule, canonical)) => {
                            if rule_key(rule) != rule_key(entry.rule) {
                                warnings.push(format!(
                                    "Overlay rule '{}' rulestring '{}' does not match builtin '{}'; ignoring rulestring",
                                    overlay.id, rulestring, entry.rulestring
                                ));
                            } else {
                                entry.rulestring = canonical;
                                entry.rulestring_raw = rulestring.trim().to_string();
                                entry.rule = rule;
                            }
                        }
                        Err(err) => warnings.push(format!(
                            "Invalid overlay rulestring '{}' for id '{}': {err}",
                            rulestring, overlay.id
                        )),
                    }
                }
                if let Some(name) = &overlay.display_name {
                    entry.name = name.clone();
                }
                if let Some(description) = &overlay.description {
                    entry.description = description.clone();
                }
                if let Some(tags) = &overlay.tags {
                    entry.tags = normalize_list(tags.clone());
                }
                if let Some(aliases) = &overlay.aliases {
                    entry.aliases = normalize_list(aliases.clone());
                }
                if let Some(params) = &overlay.default_params {
                    entry.default_params = params.clone();
                }
                if let Some(provenance) = &overlay.provenance {
                    entry.provenance = normalize_lines(provenance.clone());
                }
                if let Some(favorite) = overlay.favorite {
                    entry.favorite = favorite;
                }
                if let Some(hidden) = overlay.hidden {
                    entry.hidden = hidden;
                }
                continue;
            }
            let Some(rulestring) = overlay.rulestring.as_deref() else {
                warnings.push(format!(
                    "Overlay rule '{}' missing rulestring; skipping",
                    overlay.id
                ));
                continue;
            };
            let Some(name) = overlay.display_name.as_deref() else {
                warnings.push(format!(
                    "Overlay rule '{}' missing display_name; skipping",
                    overlay.id
                ));
                continue;
            };
            let Some(description) = overlay.description.as_deref() else {
                warnings.push(format!(
                    "Overlay rule '{}' missing description; skipping",
                    overlay.id
                ));
                continue;
            };
            let (rule, canonical) = match normalize_rulestring(rulestring) {
                Ok(rule) => rule,
                Err(err) => {
                    warnings.push(format!(
                        "Invalid overlay rulestring '{}' for id '{}': {err}",
                        rulestring, overlay.id
                    ));
                    continue;
                }
            };
            let key = rule_key(rule);
            if let Some(existing) = rule_map.get(&key) {
                let existing_id = self
                    .entries
                    .get(*existing)
                    .map(|entry| entry.id.as_str())
                    .unwrap_or("unknown");
                warnings.push(format!(
                    "Overlay rule '{}' duplicates rulestring '{}' (existing id '{}'); add as alias instead",
                    overlay.id, canonical, existing_id
                ));
                continue;
            }
            let entry = RuleEntry {
                id: overlay.id.clone(),
                name: name.to_string(),
                rule,
                rulestring: canonical,
                rulestring_raw: rulestring.trim().to_string(),
                description: description.to_string(),
                tags: normalize_list(overlay.tags.clone().unwrap_or_default()),
                aliases: normalize_list(overlay.aliases.clone().unwrap_or_default()),
                default_params: overlay.default_params.clone().unwrap_or_default(),
                provenance: normalize_lines(overlay.provenance.clone().unwrap_or_default()),
                favorite: overlay.favorite.unwrap_or(false),
                hidden: overlay.hidden.unwrap_or(false),
                source: RuleSource::User,
            };
            let idx = self.entries.len();
            id_map.insert(id_key, idx);
            rule_map.insert(key, idx);
            self.entries.push(entry);
        }
    }

    fn rebuild_indices(&mut self, warnings: &mut Vec<String>) {
        self.visible_indices.clear();
        self.id_index.clear();
        self.rule_index.clear();
        self.alias_index.clear();
        for (idx, entry) in self.entries.iter().enumerate() {
            let id_key = entry.id.to_ascii_lowercase();
            if self.id_index.insert(id_key, idx).is_some() {
                warnings.push(format!("Duplicate rule id '{}'", entry.id));
            }
            let key = rule_key(entry.rule);
            if self.rule_index.insert(key, idx).is_some() {
                warnings.push(format!(
                    "Duplicate rulestring for rule '{}': {}",
                    entry.id, entry.rulestring
                ));
            }
            for alias in &entry.aliases {
                let alias_key = alias.to_ascii_lowercase();
                self.alias_index.entry(alias_key).or_insert(idx);
            }
            if !entry.hidden {
                self.visible_indices.push(idx);
            }
        }
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

#[derive(Debug)]
pub enum RuleSelectError {
    UnknownId(String),
    Parse(RuleParseError),
}

impl std::fmt::Display for RuleSelectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleSelectError::UnknownId(value) => {
                write!(f, "unknown rule id '{}'", value)
            }
            RuleSelectError::Parse(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RuleSelectError {}

fn normalize_rulestring(text: &str) -> Result<(Rule, String), RuleParseError> {
    let rule = Rule::parse(text)?;
    let canonical = rule.to_string();
    Ok((rule, canonical))
}

fn normalize_list(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lowered = trimmed.to_ascii_lowercase();
        if seen.insert(lowered.clone()) {
            out.push(lowered);
        }
    }
    out
}

fn normalize_lines(items: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for item in items {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push(trimmed.to_string());
    }
    out
}

fn rule_key(rule: Rule) -> u32 {
    ((rule.births_mask() as u32) << 9) | (rule.survives_mask() as u32)
}

fn build_entry_from_file(
    entry: RuleFileEntry,
    source: RuleSource,
) -> Result<RuleEntry, RuleParseError> {
    let raw = entry.rulestring.trim().to_string();
    let (rule, canonical) = normalize_rulestring(&entry.rulestring)?;
    Ok(RuleEntry {
        id: entry.id,
        name: entry.display_name,
        rule,
        rulestring: canonical,
        rulestring_raw: raw,
        description: entry.description,
        tags: normalize_list(entry.tags),
        aliases: normalize_list(entry.aliases),
        default_params: entry.default_params,
        provenance: normalize_lines(entry.provenance),
        favorite: false,
        hidden: false,
        source,
    })
}

fn default_overlay_path() -> Option<PathBuf> {
    paths::config_dir().map(|dir| dir.join("rules.toml"))
}

fn read_overlay_file(path: &PathBuf) -> Result<Vec<RuleOverlay>, String> {
    let contents = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let file: RuleOverlayFile = toml::from_str(&contents).map_err(|e| e.to_string())?;
    Ok(file.rules)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_unique_and_canonical() {
        let mut warnings = Vec::new();
        let catalog = RuleCatalog::load_builtin(&mut warnings);
        assert!(warnings.is_empty());
        let mut ids = HashSet::new();
        let mut rules = HashSet::new();
        for entry in catalog.entries.iter() {
            assert!(ids.insert(entry.id.to_ascii_lowercase()));
            assert!(rules.insert(rule_key(entry.rule)));
            let parsed = Rule::parse(&entry.rulestring).expect("parse canonical");
            assert_eq!(parsed.to_string(), entry.rulestring);
        }
    }

    #[test]
    fn overlay_merges_and_adds_rules() {
        let builtin = r#"
[[rules]]
id = "base"
display_name = "Base"
rulestring = "B3/S23"
description = "Base rule"
tags = ["classic"]
aliases = ["base"]
"#;
        let overlay = r#"
[[rules]]
id = "base"
description = "Override rule"
tags = ["override"]
aliases = ["override"]
hidden = true

[[rules]]
id = "custom"
display_name = "Custom"
rulestring = "B2/S"
description = "Custom rule"
tags = ["custom"]
aliases = ["c"]
favorite = true
"#;
        let base_file: RuleFile = toml::from_str(builtin).expect("builtin parse");
        let mut entries = Vec::new();
        for entry in base_file.rules {
            entries.push(build_entry_from_file(entry, RuleSource::Builtin).unwrap());
        }
        let mut catalog = RuleCatalog::from_entries(entries);
        let overlay_file: RuleOverlayFile = toml::from_str(overlay).expect("overlay parse");
        let overlays = overlay_file.rules;
        let mut warnings = Vec::new();
        catalog.apply_overlays(&overlays, &mut warnings);
        catalog.rebuild_indices(&mut warnings);
        assert!(warnings.is_empty());
        let base = catalog.find_by_id("base").expect("base present");
        assert_eq!(base.description, "Override rule");
        assert_eq!(base.tags, vec!["override"]);
        assert!(base.hidden);
        let custom = catalog.find_by_id("custom").expect("custom present");
        assert_eq!(custom.rulestring, "B2/S");
        assert!(custom.favorite);
    }
}
