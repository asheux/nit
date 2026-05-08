use std::path::{Path, PathBuf};

use nit_utils::hashing::stable_hash_bytes;
use nit_utils::paths;

use crate::config::{GolRuleConfig, GolRulesConfig};

use super::parse_rules::parse_user_rules;
use super::toml_io::{get_bool, get_str, read_toml};

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
    let global_path = paths::config_dir().map(|p| p.join("config.toml"));
    let workspace_path = workspace_config_path(workspace_root);

    let mut warnings = Vec::new();
    let (rule, rules) = load_global(global_path.as_deref(), &mut warnings);
    let workspace_rule = load_workspace_override(workspace_path.as_deref(), &mut warnings);

    RuleConfigLoad {
        rule,
        rules,
        workspace_rule,
        global_path,
        workspace_path,
        warnings,
    }
}

fn load_global(path: Option<&Path>, warnings: &mut Vec<String>) -> (GolRuleConfig, GolRulesConfig) {
    let mut rule = GolRuleConfig::default();
    let mut rules = GolRulesConfig::default();
    let mut user_rule_warnings: Vec<String> = Vec::new();
    read_optional_toml(path, warnings, "global", |value| {
        if let Some(default) = get_str(value, &["gol", "rule", "default"]) {
            rule.default = default;
        }
        if let Some(workspace_override) = get_bool(value, &["gol", "rule", "workspace_override"]) {
            rule.workspace_override = workspace_override;
        }
        rules.user = parse_user_rules(value, &mut |w| user_rule_warnings.push(w));
    });
    warnings.append(&mut user_rule_warnings);
    (rule, rules)
}

fn load_workspace_override(path: Option<&Path>, warnings: &mut Vec<String>) -> Option<String> {
    let mut workspace_rule = None;
    read_optional_toml(path, warnings, "workspace", |value| {
        if let Some(default) = get_str(value, &["gol", "rule", "default"]) {
            workspace_rule = Some(default);
        }
    });
    workspace_rule
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

fn workspace_config_path(workspace_root: &Path) -> Option<PathBuf> {
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
