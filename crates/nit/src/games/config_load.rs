use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use anyhow::Context;
use nit_core::io as core_io;
use nit_games::{
    accelerator_run_preflight, try_select_halting_turing_machine_strategies, GamesConfig,
    NormalizedConfig, StrategySpec,
};

const DEFAULT_CONFIG_FILENAME: &str = "games.toml";

pub(super) type LoadedConfig = (PathBuf, String, NormalizedConfig);

pub(super) fn load_games_config(
    config: Option<PathBuf>,
    sidecar: Option<PathBuf>,
) -> anyhow::Result<LoadedConfig> {
    let path = config.unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_FILENAME));

    let text = core_io::load_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    let mut cfg =
        GamesConfig::from_toml_with_root(&text, path.parent()).map_err(|e| anyhow::anyhow!(e))?;

    if let Some(sidecar) = sidecar {
        let resolved = resolve_relative_path(&sidecar, path.parent());
        append_strategies_from_ndjson(&mut cfg, &resolved)?;
    }

    Ok((path, text, cfg))
}

pub(super) fn resolve_output_dir(
    config_location: &Path,
    requested: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let anchor = match config_location.parent() {
        Some(parent) if parent.is_absolute() => parent.to_path_buf(),
        Some(parent) => cwd.join(parent),
        None => cwd,
    };

    let target = requested.unwrap_or_else(|| anchor.clone());
    Ok(if target.is_absolute() {
        target
    } else {
        anchor.join(target)
    })
}

fn resolve_relative_path(candidate: &Path, anchor: Option<&Path>) -> PathBuf {
    if candidate.is_absolute() {
        return candidate.to_path_buf();
    }
    if let Some(parent) = anchor {
        return parent.join(candidate);
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(candidate)
}

pub(super) fn finalize_config(raw: NormalizedConfig) -> anyhow::Result<NormalizedConfig> {
    let normalized =
        try_select_halting_turing_machine_strategies(raw).map_err(|e| anyhow::anyhow!(e))?;

    let saving = normalized.save_data;
    let event_logging = saving && normalized.event_log.enabled;
    let history_logging = saving && normalized.history.enabled;
    accelerator_run_preflight(&normalized, event_logging, history_logging, false)
        .map_err(|e| anyhow::anyhow!(e))?;

    Ok(normalized)
}

// Blank lines skipped silently; parse errors carry source path and 1-based line number.
fn append_strategies_from_ndjson(cfg: &mut NormalizedConfig, file: &Path) -> anyhow::Result<()> {
    let handle = fs::File::open(file)
        .with_context(|| format!("failed to open strategies {}", file.display()))?;

    for (line_idx, raw) in std::io::BufReader::new(handle).lines().enumerate() {
        let raw = raw?;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let spec: StrategySpec = serde_json::from_str(line)
            .with_context(|| format!("failed to parse {} line {}", file.display(), line_idx + 1))?;
        cfg.strategies.push(spec);
    }

    Ok(())
}

pub(super) fn create_parent_dirs(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    Ok(())
}
