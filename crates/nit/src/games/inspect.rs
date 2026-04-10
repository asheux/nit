use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use nit_core::io as core_io;
use nit_games::output::RunSummary;
use nit_games::{format_strategy_introspection, introspect_strategy, StrategySpec};

use crate::cli::OutputFormat;
use crate::graph::{build_strategy_graph, render_strategy_graph_dot, write_strategy_graph_json};

pub(super) fn run_games_inspect(
    config_path: Option<PathBuf>,
    id: String,
    format: OutputFormat,
    out: Option<PathBuf>,
) -> anyhow::Result<()> {
    let (_config_path, _config_text, config) = super::load_games_config(config_path, None)?;
    let spec = resolve_strategy(&config.strategies, &id)?;
    let intro = introspect_strategy(&spec);
    let output = match format {
        OutputFormat::Json => serde_json::to_string(&intro)?,
        OutputFormat::Pretty => format_strategy_introspection(&intro).join("\n"),
    };

    if let Some(out_path) = out {
        create_parent_dirs(&out_path)?;
        fs::write(&out_path, output)
            .with_context(|| format!("failed to write {}", out_path.display()))?;
    } else {
        println!("{output}");
    }

    Ok(())
}

pub(super) fn run_games_graph(
    config_path: Option<PathBuf>,
    run_path: Option<PathBuf>,
    strategy_id: String,
    out_path: PathBuf,
) -> anyhow::Result<()> {
    let strategies = if let Some(run_path) = run_path {
        let run_text = core_io::load_to_string(&run_path)
            .with_context(|| format!("failed to read {}", run_path.display()))?;
        let summary: RunSummary = serde_json::from_str(&run_text)
            .with_context(|| format!("failed to parse {}", run_path.display()))?;
        summary.config.strategies
    } else {
        let (_config_path, _config_text, config) = super::load_games_config(config_path, None)?;
        config.strategies
    };
    let spec = resolve_strategy(&strategies, &strategy_id)?;
    let intro = introspect_strategy(&spec);
    let graph = build_strategy_graph(&intro);

    let ext = out_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");
    let is_json = ext.eq_ignore_ascii_case("json");
    let is_dot = ext.eq_ignore_ascii_case("dot") || ext.eq_ignore_ascii_case("gv");
    if !is_json && !is_dot {
        anyhow::bail!("output path must end with .json, .dot, or .gv");
    }

    create_parent_dirs(&out_path)?;

    if is_json {
        write_strategy_graph_json(&out_path, &graph)?;
    } else {
        let dot = render_strategy_graph_dot(&graph);
        fs::write(&out_path, dot)
            .with_context(|| format!("failed to write {}", out_path.display()))?;
    }

    eprintln!("Graph written: {}", out_path.display());
    Ok(())
}

fn resolve_strategy(strategies: &[StrategySpec], id: &str) -> anyhow::Result<StrategySpec> {
    strategies
        .iter()
        .find(|spec| spec.id == id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("strategy '{id}' not found"))
}

fn create_parent_dirs(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    Ok(())
}
