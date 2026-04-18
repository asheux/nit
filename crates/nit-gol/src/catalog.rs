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

use std::collections::{HashMap, HashSet};
use std::fs;

use crate::{Rule, RuleParseError};
use nit_utils::paths;

const DEFAULT_RULES_TOML: &str = include_str!("../assets/rules.toml");

// ── File-level serde types ──────────────────────────────────────────

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

// ── Public types ────────────────────────────────────────────────────

/// Optional per-rule parameters embedded in the catalog.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct RuleDefaultParams {
    #[serde(default)]
    pub warning: Option<String>,
    #[serde(default)]
    pub wrap: Option<String>,
}

/// A user-supplied overlay that can modify or add catalog entries.
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

/// Provenance of a catalog entry.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RuleSource {
    /// Shipped with the binary.
    Builtin,
    /// Added or overridden by user configuration.
    User,
}

/// A fully resolved rule entry in the catalog.
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
    /// Catalog-declared warning message for this rule, if any.
    pub fn warning(&self) -> Option<&str> {
        self.default_params.warning.as_deref()
    }
}

/// A user's current rule selection with optional catalog metadata.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SelectedRule {
    pub rule: Rule,
    pub id: Option<String>,
    pub name: Option<String>,
}

impl SelectedRule {
    /// Create a selection from a bare rule (no catalog metadata).
    pub fn from_rule(rule: Rule) -> Self {
        Self {
            rule,
            id: None,
            name: None,
        }
    }

    /// Create a selection from a named catalog entry.
    pub fn from_named(entry: &RuleEntry) -> Self {
        Self {
            rule: entry.rule,
            id: Some(entry.id.clone()),
            name: Some(entry.name.clone()),
        }
    }

    /// Return the most specific selector string for this rule.
    pub fn selector(&self) -> String {
        self.id.clone().unwrap_or_else(|| self.rule.to_string())
    }

    /// Format as `rulestring (name)` for display.
    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => format!("{} ({})", self.rule, name),
            None => self.rule.to_string(),
        }
    }

    /// Format as `name (rulestring)` for display.
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

// ── Catalog ─────────────────────────────────────────────────────────

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
        let overlay_path = paths::config_dir().map(|dir| dir.join("rules.toml"));
        if let Some(path) = overlay_path.filter(|p| p.exists()) {
            let parsed = fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| {
                    toml::from_str::<RuleOverlayFile>(&text).map_err(|e| e.to_string())
                });
            match parsed {
                Ok(file) => catalog.apply_overlays(&file.rules, &mut warnings),
                Err(err) => warnings.push(format!("Failed to parse rules overlay: {err}")),
            }
        }
        catalog.rebuild_indices(&mut warnings);
        (catalog, warnings)
    }

    /// Load built-ins, then apply additional overlays on top.
    pub fn load_with_overlays(overlays: &[RuleOverlay]) -> (Self, Vec<String>) {
        let (mut catalog, mut warnings) = Self::load();
        catalog.apply_overlays(overlays, &mut warnings);
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
        let mut catalog = Self::from_entries(entries);
        catalog.rebuild_indices(warnings);
        catalog
    }

    /// Apply a set of overlays: merge into existing entries or add new ones.
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
                try_update_rulestring(entry, overlay, warnings);
                apply_optional_fields(entry, overlay);
                continue;
            }
            if let Some(entry) = build_overlay_entry(overlay, &rule_map, &self.entries, warnings) {
                let idx = self.entries.len();
                id_map.insert(id_key, idx);
                rule_map.insert(rule_key(entry.rule), idx);
                self.entries.push(entry);
            }
        }
    }

    /// Rebuild all lookup indices from the current entries.
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

// ── Error ───────────────────────────────────────────────────────────

/// Error returned when a rule selector cannot be resolved.
#[derive(Debug)]
pub enum RuleSelectError {
    /// Selector did not match any id or alias and was not a parseable rulestring.
    UnknownId(String),
    /// Selector parsed as a rulestring candidate but the grammar rejected it.
    Parse(RuleParseError),
}

impl std::fmt::Display for RuleSelectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleSelectError::UnknownId(value) => write!(f, "unknown rule id '{value}'"),
            RuleSelectError::Parse(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RuleSelectError {}

// ── Overlay helpers ─────────────────────────────────────────────────

/// Validate and apply a rulestring override from an overlay.
///
/// Rulestrings are only applied when they belong to the same rule
/// family (same birth/survive digits); a mismatch is treated as user
/// error and logged, because silently rewriting a builtin's behavior
/// would be surprising.
fn try_update_rulestring(entry: &mut RuleEntry, overlay: &RuleOverlay, warnings: &mut Vec<String>) {
    let Some(rulestring) = overlay.rulestring.as_deref() else {
        return;
    };
    let Some((rule, canonical)) = parse_overlay_rulestring(rulestring, overlay, warnings) else {
        return;
    };
    if rule_key(rule) != rule_key(entry.rule) {
        warnings.push(format!(
            "Overlay rule '{}' rulestring '{}' does not match builtin '{}'; ignoring rulestring",
            overlay.id, rulestring, entry.rulestring
        ));
        return;
    }
    entry.rulestring = canonical;
    entry.rulestring_raw = rulestring.trim().to_string();
    entry.rule = rule;
}

/// Copy non-rulestring optional fields from an overlay into an entry.
fn apply_optional_fields(entry: &mut RuleEntry, overlay: &RuleOverlay) {
    if let Some(name) = &overlay.display_name {
        entry.name = name.clone();
    }
    if let Some(description) = &overlay.description {
        entry.description = description.clone();
    }
    if let Some(tags) = &overlay.tags {
        entry.tags = normalize_unique_lowercase(tags.clone());
    }
    if let Some(aliases) = &overlay.aliases {
        entry.aliases = normalize_unique_lowercase(aliases.clone());
    }
    if let Some(params) = &overlay.default_params {
        entry.default_params = params.clone();
    }
    if let Some(provenance) = &overlay.provenance {
        entry.provenance = trim_nonempty_lines(provenance.clone());
    }
    if let Some(favorite) = overlay.favorite {
        entry.favorite = favorite;
    }
    if let Some(hidden) = overlay.hidden {
        entry.hidden = hidden;
    }
}

/// Attempt to construct a new [`RuleEntry`] from an overlay definition.
///
/// Returns `None` and pushes diagnostics if required fields are missing,
/// the rulestring is invalid, or the rulestring duplicates an existing entry.
fn build_overlay_entry(
    overlay: &RuleOverlay,
    rule_map: &HashMap<u32, usize>,
    entries: &[RuleEntry],
    warnings: &mut Vec<String>,
) -> Option<RuleEntry> {
    let rulestring = require_field(
        overlay.rulestring.as_deref(),
        "rulestring",
        overlay,
        warnings,
    )?;
    let name = require_field(
        overlay.display_name.as_deref(),
        "display_name",
        overlay,
        warnings,
    )?;
    let description = require_field(
        overlay.description.as_deref(),
        "description",
        overlay,
        warnings,
    )?;
    let (rule, canonical) = parse_overlay_rulestring(rulestring, overlay, warnings)?;
    if let Some(&existing_idx) = rule_map.get(&rule_key(rule)) {
        let existing_id = entries
            .get(existing_idx)
            .map(|e| e.id.as_str())
            .unwrap_or("unknown");
        warnings.push(format!(
            "Overlay rule '{}' duplicates rulestring '{}' (existing id '{}'); add as alias instead",
            overlay.id, canonical, existing_id
        ));
        return None;
    }
    Some(RuleEntry {
        id: overlay.id.clone(),
        name: name.to_string(),
        rule,
        rulestring: canonical,
        rulestring_raw: rulestring.trim().to_string(),
        description: description.to_string(),
        tags: normalize_unique_lowercase(overlay.tags.clone().unwrap_or_default()),
        aliases: normalize_unique_lowercase(overlay.aliases.clone().unwrap_or_default()),
        default_params: overlay.default_params.clone().unwrap_or_default(),
        provenance: trim_nonempty_lines(overlay.provenance.clone().unwrap_or_default()),
        favorite: overlay.favorite.unwrap_or(false),
        hidden: overlay.hidden.unwrap_or(false),
        source: RuleSource::User,
    })
}

/// Parse an overlay rulestring, logging a diagnostic on failure.
fn parse_overlay_rulestring(
    rulestring: &str,
    overlay: &RuleOverlay,
    warnings: &mut Vec<String>,
) -> Option<(Rule, String)> {
    match Rule::parse(rulestring) {
        Ok(rule) => {
            let canonical = rule.to_string();
            Some((rule, canonical))
        }
        Err(err) => {
            warnings.push(format!(
                "Invalid overlay rulestring '{}' for id '{}': {err}",
                rulestring, overlay.id
            ));
            None
        }
    }
}

// ── Utility functions ───────────────────────────────────────────────

/// Return the field value or warn that a required overlay field is missing.
fn require_field<'a>(
    value: Option<&'a str>,
    field: &str,
    overlay: &RuleOverlay,
    warnings: &mut Vec<String>,
) -> Option<&'a str> {
    if value.is_none() {
        warnings.push(format!(
            "Overlay rule '{}' missing {field}; skipping",
            overlay.id
        ));
    }
    value
}

/// Normalize a string into a case-insensitive lookup key.
fn normalize_key(text: &str) -> String {
    text.trim().to_ascii_lowercase()
}

/// Trim, lowercase, and deduplicate a list of tag/alias strings.
fn normalize_unique_lowercase(items: Vec<String>) -> Vec<String> {
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

/// Trim each line and drop empty entries; preserves order and case.
fn trim_nonempty_lines(items: Vec<String>) -> Vec<String> {
    items
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Encode a rule as a compact `u32` key for index lookups.
///
/// Packs the 9-bit births mask into the high bits and the 9-bit
/// survives mask into the low bits so that every distinct rule maps
/// to a unique key without collision.
fn rule_key(rule: Rule) -> u32 {
    ((rule.births_mask() as u32) << 9) | (rule.survives_mask() as u32)
}

fn build_entry_from_file(
    raw: RuleFileEntry,
    source: RuleSource,
) -> Result<RuleEntry, RuleParseError> {
    let raw_str = raw.rulestring.trim().to_string();
    let rule = Rule::parse(&raw.rulestring)?;
    let canonical = rule.to_string();
    Ok(RuleEntry {
        id: raw.id,
        name: raw.display_name,
        rule,
        rulestring: canonical,
        rulestring_raw: raw_str,
        description: raw.description,
        tags: normalize_unique_lowercase(raw.tags),
        aliases: normalize_unique_lowercase(raw.aliases),
        default_params: raw.default_params,
        provenance: trim_nonempty_lines(raw.provenance),
        favorite: false,
        hidden: false,
        source,
    })
}

/// Concatenate searchable fields of an entry into a single haystack.
fn build_search_haystack(entry: &RuleEntry) -> String {
    let mut hay = String::new();
    hay.push_str(&entry.id);
    hay.push(' ');
    hay.push_str(&entry.name);
    hay.push(' ');
    hay.push_str(&entry.rulestring);
    hay.push(' ');
    hay.push_str(&entry.description);
    for tag in &entry.tags {
        hay.push(' ');
        hay.push_str(tag);
    }
    for alias in &entry.aliases {
        hay.push(' ');
        hay.push_str(alias);
    }
    hay
}

#[cfg(test)]
#[path = "test_modules/catalog.rs"]
mod tests;
