//! User-rule TOML parsing.
//!
//! Each `[gol.rules.user]` table entry must carry id/name/rule/description;
//! malformed entries are skipped with a positional warning so the loader can
//! present every problem at once instead of failing on the first.

use crate::config::GolUserRule;

use super::toml_io::get_array;

pub(super) fn parse_user_rules<F>(value: &toml::Value, warn: &mut F) -> Vec<GolUserRule>
where
    F: FnMut(String),
{
    let Some(arr) = get_array(value, &["gol", "rules", "user"]) else {
        return Vec::new();
    };
    arr.iter()
        .enumerate()
        .filter_map(|(idx, entry)| parse_one_rule(idx, entry, warn))
        .collect()
}

fn parse_one_rule<F>(idx: usize, entry: &toml::Value, warn: &mut F) -> Option<GolUserRule>
where
    F: FnMut(String),
{
    let Some(table) = entry.as_table() else {
        warn(format!("Rule entry {idx} is not a table; skipping"));
        return None;
    };
    let read = |key: &str| table.get(key).and_then(|v| v.as_str());
    match (read("id"), read("name"), read("rule"), read("description")) {
        (Some(id), Some(name), Some(rule), Some(description)) => Some(GolUserRule {
            id: id.to_string(),
            name: name.to_string(),
            rule: rule.to_string(),
            description: description.to_string(),
        }),
        _ => {
            warn(format!(
                "Rule entry {idx} missing required fields (id/name/rule/description); skipping"
            ));
            None
        }
    }
}
