//! TOML I/O helpers shared by `load` and `persist`.
//!
//! Writes route through a sibling temp file + rename so a crash mid-write
//! cannot leave the config truncated. Path-style getters take a `&[&str]`
//! key chain so callers can phrase nested lookups (`["gol", "rule",
//! "default"]`) without manual `.as_table()` plumbing.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

pub(super) fn read_toml(path: &Path) -> Result<toml::Value, String> {
    let contents = fs::read_to_string(path).map_err(|e| e.to_string())?;
    toml::from_str(&contents).map_err(|e| e.to_string())
}

pub(super) fn write_toml_atomic(path: &Path, value: &toml::Value) -> std::io::Result<()> {
    let contents = toml::to_string_pretty(value).map_err(std::io::Error::other)?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_else(|| "config".into());
    let tmp = path.with_file_name(format!(".{file_name}.nit.tmp"));
    {
        let mut file = File::create(&tmp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

pub(super) fn ensure_parent_dir(path: &Path) -> std::io::Result<()> {
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

pub(super) fn get_str(value: &toml::Value, path: &[&str]) -> Option<String> {
    get_value(value, path)
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

pub(super) fn get_bool(value: &toml::Value, path: &[&str]) -> Option<bool> {
    get_value(value, path).and_then(toml::Value::as_bool)
}

pub(super) fn get_array<'a>(value: &'a toml::Value, path: &[&str]) -> Option<&'a Vec<toml::Value>> {
    get_value(value, path).and_then(|v| v.as_array())
}

pub(super) fn set_str(value: &mut toml::Value, path: &[&str], text: &str) {
    let Some((leaf, parents)) = path.split_last() else {
        return;
    };
    let mut current = value;
    for key in parents {
        let table = ensure_table(current);
        current = table
            .entry((*key).to_string())
            .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    }
    ensure_table(current).insert((*leaf).to_string(), toml::Value::String(text.to_string()));
}

fn ensure_table(value: &mut toml::Value) -> &mut toml::value::Table {
    if !value.is_table() {
        *value = toml::Value::Table(toml::value::Table::new());
    }
    value.as_table_mut().expect("just promoted to a table")
}
