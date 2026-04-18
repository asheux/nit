//! Normalization and lookup helpers shared by catalog loading and overlay application.

use std::collections::HashSet;

use super::types::RuleEntry;
use crate::Rule;

/// Normalize a string into a case-insensitive lookup key.
pub(super) fn normalize_key(text: &str) -> String {
    text.trim().to_ascii_lowercase()
}

/// Trim, lowercase, and deduplicate a list of tag/alias strings.
pub(super) fn normalize_unique_lowercase(items: Vec<String>) -> Vec<String> {
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
pub(super) fn trim_nonempty_lines(items: Vec<String>) -> Vec<String> {
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
pub(super) fn rule_key(rule: Rule) -> u32 {
    ((rule.births_mask() as u32) << 9) | (rule.survives_mask() as u32)
}

/// Concatenate searchable fields of an entry into a single haystack.
pub(super) fn build_search_haystack(entry: &RuleEntry) -> String {
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
