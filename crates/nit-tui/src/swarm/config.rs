use std::fs;
use std::path::Path;

use super::{Gate, SwarmDagValidationMode};

fn load_workspace_toml(workspace_root: &Path) -> Result<Option<toml::Value>, String> {
    let path = workspace_root.join(".nit").join("config.toml");
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path)
        .map_err(|err| format!("failed reading {}: {err}", path.display()))?;
    let value = toml::from_str::<toml::Value>(&contents)
        .map_err(|err| format!("failed parsing {}: {err}", path.display()))?;
    Ok(Some(value))
}

pub(super) fn read_workspace_gate_default(workspace_root: &Path) -> Result<Option<String>, String> {
    let Some(value) = load_workspace_toml(workspace_root)? else {
        return Ok(None);
    };
    Ok(value
        .get("swarm")
        .and_then(|value| value.get("gates"))
        .and_then(|value| value.get("default"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase()))
}

/// Read project-specific custom gate definitions from `.nit/config.toml`.
/// Schema:
///
/// ```toml
/// [[swarm.gates.custom]]
/// name = "fmt"
/// command = "just fmt-check"
/// scoped_command = "just fmt-check-crates {cargo_packages}"  # optional
///
/// [[swarm.gates.custom]]
/// name = "test"
/// command = "just test"
/// scoped_command = "just test-crates {cargo_packages}"
/// ```
///
/// Returns `Ok(None)` when no custom gates are configured, `Ok(Some(gates))`
/// when at least one is defined, or `Err` on a malformed config file. When
/// custom gates are returned, they fully replace the auto-detected language
/// bundle — the project owner is asserting "these are my gates".
pub(super) fn read_workspace_custom_gates(
    workspace_root: &Path,
) -> Result<Option<Vec<Gate>>, String> {
    let Some(value) = load_workspace_toml(workspace_root)? else {
        return Ok(None);
    };
    let Some(array) = value
        .get("swarm")
        .and_then(|value| value.get("gates"))
        .and_then(|value| value.get("custom"))
        .and_then(|value| value.as_array())
    else {
        return Ok(None);
    };
    if array.is_empty() {
        return Ok(None);
    }
    let mut gates = Vec::with_capacity(array.len());
    for (idx, entry) in array.iter().enumerate() {
        let table = entry
            .as_table()
            .ok_or_else(|| format!("swarm.gates.custom[{idx}] must be a table"))?;
        let name = table
            .get("name")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("swarm.gates.custom[{idx}].name is required"))?
            .to_string();
        let command = table
            .get("command")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("swarm.gates.custom[{idx}].command is required"))?
            .to_string();
        let scoped_command = table
            .get("scoped_command")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string());
        gates.push(Gate {
            name,
            command,
            scoped_command,
        });
    }
    Ok(Some(gates))
}

pub(super) fn read_workspace_dag_validation_mode(
    workspace_root: &Path,
) -> Result<Option<SwarmDagValidationMode>, String> {
    let Some(value) = load_workspace_toml(workspace_root)? else {
        return Ok(None);
    };
    let Some(mode) = value
        .get("swarm")
        .and_then(|value| value.get("dag_validation"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let mode = mode.to_ascii_lowercase();
    if mode == "strict" || mode == "hard-fail" || mode == "hard_fail" || mode == "hardfail" {
        return Ok(Some(SwarmDagValidationMode::Strict));
    }
    if mode == "repair"
        || mode == "best-effort"
        || mode == "best_effort"
        || mode == "auto-repair"
        || mode == "auto_repair"
    {
        return Ok(Some(SwarmDagValidationMode::Repair));
    }

    Err(format!(
        "invalid swarm.dag_validation value '{mode}' (expected 'strict' or 'repair')"
    ))
}
