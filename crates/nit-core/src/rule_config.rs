use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use nit_utils::hashing::stable_hash_bytes;
use nit_utils::paths;

use crate::config::{GolRuleConfig, GolRulesConfig, GolUserRule};

#[derive(Clone, Debug)]
pub struct RuleConfigLoad {
    pub rule: GolRuleConfig,
    pub rules: GolRulesConfig,
    pub workspace_rule: Option<String>,
    pub global_path: Option<PathBuf>,
    pub workspace_path: Option<PathBuf>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RulePersistence {
    pub global_path: Option<PathBuf>,
    pub workspace_path: Option<PathBuf>,
    pub workspace_override: bool,
}

impl Default for RulePersistence {
    fn default() -> Self {
        Self {
            global_path: None,
            workspace_path: None,
            workspace_override: true,
        }
    }
}

pub fn load_rule_config(workspace_root: &Path) -> RuleConfigLoad {
    let mut warnings = Vec::new();
    let global_path = paths::config_dir().map(|p| p.join("config.toml"));
    let workspace_path = workspace_config_path(workspace_root);

    let mut rule = GolRuleConfig::default();
    let mut rules = GolRulesConfig::default();

    if let Some(path) = global_path.as_ref().filter(|p| p.exists()) {
        match read_toml(path) {
            Ok(value) => {
                if let Some(default) = get_str(&value, &["gol", "rule", "default"]) {
                    rule.default = default;
                }
                if let Some(workspace_override) =
                    get_bool(&value, &["gol", "rule", "workspace_override"])
                {
                    rule.workspace_override = workspace_override;
                }
                rules.user = parse_user_rules(&value, &mut warnings);
            }
            Err(err) => warnings.push(format!("Failed to parse global config: {err}")),
        }
    }

    let mut workspace_rule = None;
    if let Some(path) = workspace_path.as_ref().filter(|p| p.exists()) {
        match read_toml(path) {
            Ok(value) => {
                if let Some(default) = get_str(&value, &["gol", "rule", "default"]) {
                    workspace_rule = Some(default);
                }
            }
            Err(err) => warnings.push(format!("Failed to parse workspace config: {err}")),
        }
    }

    RuleConfigLoad {
        rule,
        rules,
        workspace_rule,
        global_path,
        workspace_path,
        warnings,
    }
}

pub fn persist_rule_selection(
    persistence: &RulePersistence,
    selector: &str,
) -> std::io::Result<Option<PathBuf>> {
    let target = if persistence.workspace_override {
        persistence
            .workspace_path
            .clone()
            .or_else(|| persistence.global_path.clone())
    } else {
        persistence.global_path.clone()
    };
    let Some(path) = target else {
        return Ok(None);
    };
    let mut value = if path.exists() {
        read_toml(&path).unwrap_or_else(|_| toml::Value::Table(toml::value::Table::new()))
    } else {
        toml::Value::Table(toml::value::Table::new())
    };
    set_str(&mut value, &["gol", "rule", "default"], selector);
    ensure_parent_dir(&path)?;
    write_toml_atomic(&path, &value)?;
    Ok(Some(path))
}

fn workspace_config_path(workspace_root: &Path) -> Option<PathBuf> {
    let local = workspace_root.join(".nit").join("config.toml");
    if local.exists() {
        return Some(local);
    }
    if let Some(base) = paths::config_dir() {
        let key = workspace_root.to_string_lossy();
        let hash = stable_hash_bytes(key.as_bytes());
        return Some(base.join("workspaces").join(format!("{:016x}.toml", hash)));
    }
    Some(local)
}

fn parse_user_rules(value: &toml::Value, warnings: &mut Vec<String>) -> Vec<GolUserRule> {
    let Some(arr) = get_array(value, &["gol", "rules", "user"]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (idx, entry) in arr.iter().enumerate() {
        let Some(table) = entry.as_table() else {
            warnings.push(format!("Rule entry {idx} is not a table; skipping"));
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
            _ => warnings.push(format!(
                "Rule entry {idx} missing required fields (id/name/rule/description); skipping"
            )),
        }
    }
    out
}

fn read_toml(path: &Path) -> Result<toml::Value, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|e| e.to_string())?;
    toml::from_str(&contents).map_err(|e| e.to_string())
}

fn write_toml_atomic(path: &Path, value: &toml::Value) -> std::io::Result<()> {
    let contents = toml::to_string_pretty(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let tmp = temp_path(path);
    {
        let mut file = File::create(&tmp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_else(|| "config".into());
    let mut tmp_name = String::from(".");
    tmp_name.push_str(&file_name);
    tmp_name.push_str(".nit.tmp");
    path.with_file_name(tmp_name)
}

fn ensure_parent_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn get_value<'a>(value: &'a toml::Value, path: &[&str]) -> Option<&'a toml::Value> {
    let mut current = value;
    for key in path {
        current = current.as_table()?.get(*key)?;
    }
    Some(current)
}

fn get_str(value: &toml::Value, path: &[&str]) -> Option<String> {
    get_value(value, path)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn get_bool(value: &toml::Value, path: &[&str]) -> Option<bool> {
    get_value(value, path).and_then(|v| v.as_bool())
}

fn get_array<'a>(value: &'a toml::Value, path: &[&str]) -> Option<&'a Vec<toml::Value>> {
    get_value(value, path).and_then(|v| v.as_array())
}

fn set_str(value: &mut toml::Value, path: &[&str], text: &str) {
    if path.is_empty() {
        return;
    }
    let mut current = value;
    for key in &path[..path.len() - 1] {
        let table = ensure_table(current);
        current = table
            .entry((*key).to_string())
            .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    }
    let table = ensure_table(current);
    table.insert(
        path[path.len() - 1].to_string(),
        toml::Value::String(text.to_string()),
    );
}

fn ensure_table(value: &mut toml::Value) -> &mut toml::value::Table {
    if !value.is_table() {
        *value = toml::Value::Table(toml::value::Table::new());
    }
    value.as_table_mut().expect("table exists")
}
