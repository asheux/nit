use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub(super) fn read_toml(path: &Path) -> Result<toml::Value, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|e| e.to_string())?;
    toml::from_str(&contents).map_err(|e| e.to_string())
}

pub(super) fn write_toml_atomic(path: &Path, value: &toml::Value) -> std::io::Result<()> {
    let contents = toml::to_string_pretty(value).map_err(std::io::Error::other)?;
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
        .map(|s| s.to_string())
}

pub(super) fn get_bool(value: &toml::Value, path: &[&str]) -> Option<bool> {
    get_value(value, path).and_then(|v| v.as_bool())
}

pub(super) fn get_array<'a>(value: &'a toml::Value, path: &[&str]) -> Option<&'a Vec<toml::Value>> {
    get_value(value, path).and_then(|v| v.as_array())
}

pub(super) fn set_str(value: &mut toml::Value, path: &[&str], text: &str) {
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
