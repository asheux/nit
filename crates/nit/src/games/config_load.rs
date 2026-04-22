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

/// Load and parse a games config, optionally appending strategies from an NDJSON sidecar.
pub(super) fn load_games_config(
    toml_source: Option<PathBuf>,
    sidecar_source: Option<PathBuf>,
) -> anyhow::Result<LoadedConfig> {
    let canonical_config_path =
        toml_source.unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_FILENAME));

    let raw_toml_content = core_io::load_to_string(&canonical_config_path)
        .with_context(|| format!("failed to read {}", canonical_config_path.display()))?;

    let mut parsed_config =
        GamesConfig::from_toml_with_root(&raw_toml_content, canonical_config_path.parent())
            .map_err(|config_parse_failure| anyhow::anyhow!(config_parse_failure))?;

    if let Some(ndjson_sidecar) = sidecar_source {
        let absolute_sidecar_path =
            resolve_relative_path(&ndjson_sidecar, canonical_config_path.parent());
        append_strategies_from_ndjson(&mut parsed_config, &absolute_sidecar_path)?;
    }

    Ok((canonical_config_path, raw_toml_content, parsed_config))
}

/// Resolve the output base directory relative to the config file's parent.
pub(super) fn resolve_output_dir(
    config_location: &Path,
    user_specified_dir: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let anchor_directory = absolutize_parent(config_location.parent(), &cwd);

    let resolved_destination = user_specified_dir.unwrap_or_else(|| anchor_directory.clone());
    Ok(if resolved_destination.is_absolute() {
        resolved_destination
    } else {
        anchor_directory.join(resolved_destination)
    })
}

fn absolutize_parent(optional_base: Option<&Path>, fallback_cwd: &Path) -> PathBuf {
    match optional_base {
        Some(absolute_base) if absolute_base.is_absolute() => absolute_base.to_path_buf(),
        Some(relative_base) => fallback_cwd.join(relative_base),
        None => fallback_cwd.to_path_buf(),
    }
}

fn resolve_relative_path(candidate_path: &Path, resolution_anchor: Option<&Path>) -> PathBuf {
    if candidate_path.is_absolute() {
        return candidate_path.to_path_buf();
    }
    if let Some(parent_directory) = resolution_anchor {
        return parent_directory.join(candidate_path);
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(candidate_path)
}

/// Apply TM strategy selection and accelerator validation before tournament execution.
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

/// Load strategy specs from an NDJSON file and append them to the config.
///
/// Blank lines are silently skipped; parse errors include the source path and line number.
fn append_strategies_from_ndjson(
    target_config: &mut NormalizedConfig,
    sidecar_file: &Path,
) -> anyhow::Result<()> {
    let opened_handle = fs::File::open(sidecar_file)
        .with_context(|| format!("failed to open strategies {}", sidecar_file.display()))?;

    let line_reader = std::io::BufReader::new(opened_handle);
    for (line_number, raw_line_result) in line_reader.lines().enumerate() {
        let Some(parsed_strategy) = parse_ndjson_line(sidecar_file, line_number, raw_line_result?)?
        else {
            continue;
        };
        target_config.strategies.push(parsed_strategy);
    }

    Ok(())
}

fn parse_ndjson_line(
    origin_file: &Path,
    line_number: usize,
    input_text: String,
) -> anyhow::Result<Option<StrategySpec>> {
    let stripped_line = input_text.trim();
    if stripped_line.is_empty() {
        return Ok(None);
    }
    let deserialized_spec: StrategySpec =
        serde_json::from_str(stripped_line).with_context(|| {
            format!(
                "failed to parse {} line {}",
                origin_file.display(),
                line_number + 1,
            )
        })?;
    Ok(Some(deserialized_spec))
}

/// Create the parent directory of `path` if it has one and is not the empty path.
pub(super) fn create_parent_dirs(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    Ok(())
}
