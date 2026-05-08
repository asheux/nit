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
        // workspace_override defaults on so a per-project selection wins over
        // the user's global default; bootstrap clears it for setups that want
        // a single shared config file.
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
    let mut value = path
        .exists()
        .then(|| read_toml(&path).ok())
        .flatten()
        .unwrap_or_else(|| toml::Value::Table(toml::value::Table::new()));
    set_str(&mut value, &["gol", "rule", "default"], selector);
    ensure_parent_dir(&path)?;
    write_toml_atomic(&path, &value)?;
    Ok(Some(path))
}
