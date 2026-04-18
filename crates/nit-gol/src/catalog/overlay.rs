//! Overlay merge logic and TOML deserialization for catalog loading.

use std::collections::{HashMap, HashSet};

use super::rule_key;
use super::types::{RuleDefaultParams, RuleEntry, RuleOverlay, RuleSource};
use crate::{Rule, RuleParseError};

/// Trim, lowercase, and deduplicate tag/alias strings, preserving first-occurrence order.
fn normalize_unique_lowercase(items: Vec<String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    items
        .into_iter()
        .filter_map(|raw| {
            let lowered = raw.trim().to_ascii_lowercase();
            if lowered.is_empty() || !seen.insert(lowered.clone()) {
                return None;
            }
            Some(lowered)
        })
        .collect()
}

/// Trim each line and drop empties; preserves order and original case.
fn trim_nonempty_lines(items: Vec<String>) -> Vec<String> {
    items
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Root structure for the built-in rules TOML asset.
#[derive(Clone, Debug, serde::Deserialize)]
pub(super) struct RuleFile {
    #[serde(default)]
    pub rules: Vec<RuleFileEntry>,
}

/// One entry in the built-in rules TOML asset.
#[derive(Clone, Debug, serde::Deserialize)]
pub(super) struct RuleFileEntry {
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

/// Root structure for a user-overlay TOML file.
#[derive(Clone, Debug, serde::Deserialize)]
pub(super) struct RuleOverlayFile {
    #[serde(default)]
    pub rules: Vec<RuleOverlay>,
}

pub(super) fn build_entry_from_file(
    raw: RuleFileEntry,
    source: RuleSource,
) -> Result<RuleEntry, RuleParseError> {
    let raw_rulestring = raw.rulestring.trim().to_string();
    let rule = Rule::parse(&raw.rulestring)?;
    Ok(RuleEntry {
        id: raw.id,
        name: raw.display_name,
        rule,
        rulestring: rule.to_string(),
        rulestring_raw: raw_rulestring,
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

/// Apply a set of overlays: merge into existing entries (matched by
/// case-insensitive id) or add new user entries when the id is unknown.
pub(super) fn apply_overlays(
    entries: &mut Vec<RuleEntry>,
    overlays: &[RuleOverlay],
    warnings: &mut Vec<String>,
) {
    let (mut id_map, mut rule_map) = seed_lookup_maps(entries);
    for overlay in overlays {
        let id_key = overlay.id.to_ascii_lowercase();
        if let Some(&idx) = id_map.get(&id_key) {
            try_update_rulestring(&mut entries[idx], overlay, warnings);
            apply_optional_fields(&mut entries[idx], overlay);
            continue;
        }
        let Some(entry) = build_overlay_entry(overlay, &rule_map, entries, warnings) else {
            continue;
        };
        let idx = entries.len();
        id_map.insert(id_key, idx);
        rule_map.insert(rule_key(entry.rule), idx);
        entries.push(entry);
    }
}

fn seed_lookup_maps(entries: &[RuleEntry]) -> (HashMap<String, usize>, HashMap<u32, usize>) {
    let mut id_map = HashMap::with_capacity(entries.len());
    let mut rule_map = HashMap::with_capacity(entries.len());
    for (idx, entry) in entries.iter().enumerate() {
        id_map.insert(entry.id.to_ascii_lowercase(), idx);
        rule_map.insert(rule_key(entry.rule), idx);
    }
    (id_map, rule_map)
}

/// Apply an overlay's rulestring override only when it preserves the
/// rule's birth/survive digits. A mismatched rulestring is rejected
/// with a warning because silently rewriting a builtin's behavior via
/// an overlay would be too surprising to justify.
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

/// Copy each optional overlay field onto `entry` when the overlay
/// provides a value; `None` means "inherit the existing value".
fn apply_optional_fields(entry: &mut RuleEntry, overlay: &RuleOverlay) {
    macro_rules! merge_ref {
        ($src:ident => $dst:ident) => {
            if let Some(value) = overlay.$src.as_ref() {
                entry.$dst = value.clone();
            }
        };
        ($src:ident => $dst:ident via $transform:path) => {
            if let Some(value) = overlay.$src.as_ref() {
                entry.$dst = $transform(value.clone());
            }
        };
    }
    macro_rules! merge_copy {
        ($field:ident) => {
            if let Some(value) = overlay.$field {
                entry.$field = value;
            }
        };
    }
    merge_ref!(display_name => name);
    merge_ref!(description => description);
    merge_ref!(tags => tags via normalize_unique_lowercase);
    merge_ref!(aliases => aliases via normalize_unique_lowercase);
    merge_ref!(default_params => default_params);
    merge_ref!(provenance => provenance via trim_nonempty_lines);
    merge_copy!(favorite);
    merge_copy!(hidden);
}

/// Construct a brand-new [`RuleEntry`] from an overlay definition.
///
/// Returns `None` and appends diagnostics when required fields are
/// missing, the rulestring is invalid, or the rulestring duplicates an
/// existing catalog entry (add it as an alias instead).
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

fn parse_overlay_rulestring(
    rulestring: &str,
    overlay: &RuleOverlay,
    warnings: &mut Vec<String>,
) -> Option<(Rule, String)> {
    match Rule::parse(rulestring) {
        Ok(rule) => Some((rule, rule.to_string())),
        Err(err) => {
            warnings.push(format!(
                "Invalid overlay rulestring '{}' for id '{}': {err}",
                rulestring, overlay.id
            ));
            None
        }
    }
}

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
