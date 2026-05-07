use std::path::PathBuf;

use super::toml_io::{ensure_parent_dir, read_toml, set_str, write_toml_atomic};

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

impl RulePersistence {
    pub fn target_path(&self) -> Option<PathBuf> {
        if self.workspace_override {
            self.workspace_path
                .clone()
                .or_else(|| self.global_path.clone())
        } else {
            self.global_path.clone()
        }
    }
}

pub fn persist_rule_selection(
    persistence: &RulePersistence,
    selector: &str,
) -> std::io::Result<Option<PathBuf>> {
    let Some(path) = persistence.target_path() else {
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
