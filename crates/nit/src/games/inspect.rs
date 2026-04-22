use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use nit_core::io as core_io;
use nit_games::output::RunSummary;
use nit_games::{format_strategy_introspection, introspect_strategy, StrategySpec};

use crate::cli::{GraphArgs, InspectArgs, OutputFormat};
use crate::graph::{build_strategy_graph, render_strategy_graph_dot, write_strategy_graph_json};

use super::create_parent_dirs;

enum GraphFormat {
    Json,
    Dot,
}

pub(super) fn run_games_inspect(args: InspectArgs) -> anyhow::Result<()> {
    let InspectArgs {
        config,
        id,
        format,
        out,
    } = args;

    let (_, _, parsed) = super::load_games_config(config, None)?;
    let spec = resolve_strategy(&parsed.strategies, &id)?;
    let intro = introspect_strategy(&spec);
    let rendered = match format {
        OutputFormat::Json => serde_json::to_string(&intro)?,
        OutputFormat::Pretty => format_strategy_introspection(&intro).join("\n"),
    };

    if let Some(out_path) = out {
        create_parent_dirs(&out_path)?;
        fs::write(&out_path, rendered)
            .with_context(|| format!("failed to write {}", out_path.display()))?;
    } else {
        println!("{rendered}");
    }

    Ok(())
}

pub(super) fn run_games_graph(args: GraphArgs) -> anyhow::Result<()> {
    let GraphArgs {
        config,
        run,
        id,
        out,
    } = args;

    let strategies = load_strategies_for_graph(config, run)?;
    let spec = resolve_strategy(&strategies, &id)?;
    let intro = introspect_strategy(&spec);
    let graph = build_strategy_graph(&intro);

    let kind = detect_graph_format(&out)?;
    create_parent_dirs(&out)?;

    match kind {
        GraphFormat::Json => write_strategy_graph_json(&out, &graph)?,
        GraphFormat::Dot => {
            let dot = render_strategy_graph_dot(&graph);
            fs::write(&out, dot).with_context(|| format!("failed to write {}", out.display()))?;
        }
    }

    eprintln!("Graph written: {}", out.display());
    Ok(())
}

fn load_strategies_for_graph(
    config_path: Option<PathBuf>,
    run_path: Option<PathBuf>,
) -> anyhow::Result<Vec<StrategySpec>> {
    if let Some(run_path) = run_path {
        let raw = core_io::load_to_string(&run_path)
            .with_context(|| format!("failed to read {}", run_path.display()))?;
        let summary: RunSummary = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", run_path.display()))?;
        return Ok(summary.config.strategies);
    }
    let (_, _, parsed) = super::load_games_config(config_path, None)?;
    Ok(parsed.strategies)
}

fn detect_graph_format(out_path: &Path) -> anyhow::Result<GraphFormat> {
    let ext = out_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");
    if ext.eq_ignore_ascii_case("json") {
        Ok(GraphFormat::Json)
    } else if ext.eq_ignore_ascii_case("dot") || ext.eq_ignore_ascii_case("gv") {
        Ok(GraphFormat::Dot)
    } else {
        anyhow::bail!("output path must end with .json, .dot, or .gv")
    }
}

fn resolve_strategy(strategies: &[StrategySpec], id: &str) -> anyhow::Result<StrategySpec> {
    strategies
        .iter()
        .find(|spec| spec.id == id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("strategy '{id}' not found"))
}
