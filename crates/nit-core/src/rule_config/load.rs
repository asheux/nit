use std::path::{Path, PathBuf};

use nit_utils::hashing::stable_hash_bytes;
use nit_utils::paths;

use crate::config::{GolRuleConfig, GolRulesConfig, GolUserRule};

use super::toml_io::{get_array, get_bool, get_str, read_toml};

#[derive(Clone, Debug)]
pub struct RuleConfigLoad {
    pub rule: GolRuleConfig,
    pub rules: GolRulesConfig,
    pub workspace_rule: Option<String>,
    pub global_path: Option<PathBuf>,
    pub workspace_path: Option<PathBuf>,
    pub warnings: Vec<String>,
}

pub fn load_rule_config(workspace_root: &Path) -> RuleConfigLoad {
    let mut warnings = Vec::new();
    let global_path = paths::config_dir().map(|p| p.join("config.toml"));
    let workspace_path = workspace_config_path(workspace_root);

    let mut rule = GolRuleConfig::default();
    let mut rules = GolRulesConfig::default();
    let mut user_rule_warnings: Vec<String> = Vec::new();

    read_optional_toml(global_path.as_deref(), &mut warnings, "global", |value| {
        if let Some(default) = get_str(value, &["gol", "rule", "default"]) {
            rule.default = default;
        }
        if let Some(workspace_override) = get_bool(value, &["gol", "rule", "workspace_override"]) {
            rule.workspace_override = workspace_override;
        }
        rules.user = parse_user_rules(value, &mut |w| user_rule_warnings.push(w));
    });
    warnings.append(&mut user_rule_warnings);

    let mut workspace_rule = None;
    read_optional_toml(
        workspace_path.as_deref(),
        &mut warnings,
        "workspace",
        |value| {
            if let Some(default) = get_str(value, &["gol", "rule", "default"]) {
                workspace_rule = Some(default);
            }
        },
    );

    RuleConfigLoad {
        rule,
        rules,
        workspace_rule,
        global_path,
        workspace_path,
        warnings,
    }
}

/// If `path` exists, parse it as TOML and call `on_value`; otherwise no-op.
/// Parse errors are appended to `warnings` with a `Failed to parse <label> config:`
/// prefix (callers grep on that prefix).
fn read_optional_toml<F>(
    path: Option<&Path>,
    warnings: &mut Vec<String>,
    label: &str,
    mut on_value: F,
) where
    F: FnMut(&toml::Value),
{
    let Some(path) = path.filter(|p| p.exists()) else {
        return;
    };
    match read_toml(path) {
        Ok(value) => on_value(&value),
        Err(err) => warnings.push(format!("Failed to parse {label} config: {err}")),
    }
}

pub(super) fn workspace_config_path(workspace_root: &Path) -> Option<PathBuf> {
    let local = workspace_root.join(".nit").join("config.toml");
    if local.exists() {
        return Some(local);
    }
    if let Some(base) = paths::config_dir() {
        let key = workspace_root.to_string_lossy();
        let hash = stable_hash_bytes(key.as_bytes());
        return Some(base.join("workspaces").join(format!("{hash:016x}.toml")));
    }
    Some(local)
}

fn parse_user_rules<F>(value: &toml::Value, warn: &mut F) -> Vec<GolUserRule>
where
    F: FnMut(String),
{
    let Some(arr) = get_array(value, &["gol", "rules", "user"]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (idx, entry) in arr.iter().enumerate() {
        let Some(table) = entry.as_table() else {
            warn(format!("Rule entry {idx} is not a table; skipping"));
            continue;
        };
        let id = table.get("id").and_then(|v| v.as_str());
        let name = table.get("name").and_then(|v| v.as_str());
        let rule = table.get("rule").and_then(|v| v.as_str());
        let description = table.get("description").and_then(|v| v.as_str());
        match (id, name, rule, description) {
            (Some(id), Some(name), Some(rule), Some(description)) => out.push(GolUserRule {
                id: id.to_string(),
                name: name.to_string(),
                rule: rule.to_string(),
                description: description.to_string(),
            }),
            _ => warn(format!(
                "Rule entry {idx} missing required fields (id/name/rule/description); skipping"
            )),
        }
    }
    out
}
