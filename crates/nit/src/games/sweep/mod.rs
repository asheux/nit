//! Parameter sweep runner: executes tournaments across a Cartesian grid of game parameters.
//!
//! Each grid cell is a complete IPD tournament with its own seed, config, and output directory.
//! Results are aggregated per-strategy with mean scores, standard deviation, and top-1 counts.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use nit_games::config::EngineMode;
use nit_games::events::EventWriter;
use nit_games::{NormalizedConfig, ScoreAggregation};
use nit_utils::hashing::stable_hash_bytes;

use crate::cli::{OutputFormat, SweepArgs};

mod cell;
mod grid;
mod summary;

use cell::{run_sweep_cell, SweepAccumulators, SweepContext};
use grid::{build_cartesian_grid, resolve_parameter_grids, GridCell, ParameterGrids};
use summary::{
    compute_sweep_aggregates, SweepAggregate, SweepCellSummary, SweepGrid, SweepSummary,
    SWEEP_SCHEMA_VERSION,
};

// Re-import games-level helpers so descendant submodules (cell.rs) can
// reach them through `super::`.
use super::{execute_tournament, finalize_config, write_run_artifacts};

struct SweepPlan {
    toml_source: PathBuf,
    raw_toml: String,
    parsed_config: NormalizedConfig,
    timestamp: String,
    seed: u64,
    dimensions: ParameterGrids,
    cells_root: PathBuf,
    report_path: PathBuf,
}

pub(super) fn run_games_sweep(args: SweepArgs) -> anyhow::Result<()> {
    let SweepArgs {
        config: config_path,
        strategies: strategies_path,
        out: out_dir,
        seed: seed_override,
        rounds,
        noise,
        repetitions,
        payoff_preset,
        payoff_r,
        payoff_s,
        payoff_t,
        payoff_p,
        force,
        output,
    } = args;
    let format = output.format;
    let quiet = output.quiet;
    let verbose = output.verbose;

    let plan = build_sweep_plan(
        config_path,
        strategies_path,
        out_dir,
        seed_override,
        rounds,
        noise,
        repetitions,
        payoff_preset.as_deref(),
        payoff_r,
        payoff_s,
        payoff_t,
        payoff_p,
    )?;

    let scoring_mode = plan.parsed_config.engine.score_aggregation;
    let adjusted = plan.parsed_config.engine.complexity_cost.enabled;
    let grid = build_cartesian_grid(&plan.dimensions);

    let (cells, totals) = execute_all_cells(&plan, &grid, scoring_mode, adjusted, force, verbose)?;

    let report_path = plan.report_path.clone();
    let report = assemble_summary(plan, payoff_preset, cells, totals, scoring_mode, adjusted);

    persist_summary(&report, &report_path, format, quiet, verbose)
}

#[allow(clippy::too_many_arguments)]
fn build_sweep_plan(
    config_path: Option<PathBuf>,
    strategies_path: Option<PathBuf>,
    out_dir: Option<PathBuf>,
    seed_override: Option<u64>,
    rounds: Vec<u32>,
    noise: Vec<f32>,
    repetitions: Vec<u32>,
    payoff_preset: Option<&str>,
    payoff_r: Vec<i32>,
    payoff_s: Vec<i32>,
    payoff_t: Vec<i32>,
    payoff_p: Vec<i32>,
) -> anyhow::Result<SweepPlan> {
    let (toml_source, raw_toml, mut parsed_config) =
        super::load_games_config(config_path, strategies_path)?;

    let timestamp = EventWriter::timestamp();
    let seed = seed_override
        .or(parsed_config.seed)
        .unwrap_or_else(|| stable_hash_bytes(format!("{timestamp}\n{raw_toml}").as_bytes()));
    parsed_config.seed = Some(seed);
    parsed_config.engine.mode = EngineMode::Batch;

    let dimensions = resolve_parameter_grids(
        &parsed_config,
        rounds,
        noise,
        repetitions,
        payoff_preset,
        payoff_r,
        payoff_s,
        payoff_t,
        payoff_p,
    )?;

    let output_root = super::resolve_output_dir(&toml_source, out_dir)?;
    let sanitized = timestamp.replace(':', "-");
    let sweep_dir = output_root
        .join("runs")
        .join("games")
        .join("sweeps")
        .join(format!("{sanitized}__seed-{seed}"));
    let cells_root = sweep_dir.join("cells");
    fs::create_dir_all(&cells_root)
        .with_context(|| format!("failed to create {}", cells_root.display()))?;

    let report_path = sweep_dir.join("sweep_summary.json");

    Ok(SweepPlan {
        toml_source,
        raw_toml,
        parsed_config,
        timestamp,
        seed,
        dimensions,
        cells_root,
        report_path,
    })
}

fn execute_all_cells(
    plan: &SweepPlan,
    grid: &[GridCell],
    scoring_mode: ScoreAggregation,
    adjusted: bool,
    force: bool,
    verbose: bool,
) -> anyhow::Result<(Vec<SweepCellSummary>, SweepAccumulators)> {
    let context = SweepContext {
        base_config: &plan.parsed_config,
        config_text: &plan.raw_toml,
        base_seed: plan.seed,
        timestamp: &plan.timestamp,
        cells_root: &plan.cells_root,
        force,
        verbose,
        score_aggregation: scoring_mode,
        adjusted_scores: adjusted,
    };
    let mut totals = SweepAccumulators::default();
    let mut completed = Vec::with_capacity(grid.len());
    for (ordinal, point) in grid.iter().enumerate() {
        completed.push(run_sweep_cell(&context, &mut totals, ordinal + 1, point)?);
    }
    Ok((completed, totals))
}

fn assemble_summary(
    plan: SweepPlan,
    payoff_preset: Option<String>,
    cells: Vec<SweepCellSummary>,
    totals: SweepAccumulators,
    scoring_mode: ScoreAggregation,
    adjusted: bool,
) -> SweepSummary {
    let SweepPlan {
        toml_source,
        timestamp,
        seed,
        dimensions,
        ..
    } = plan;
    let strategies = compute_sweep_aggregates(totals.scores_by_strategy, totals.top_counts);
    SweepSummary {
        schema_version: SWEEP_SCHEMA_VERSION,
        timestamp,
        seed,
        config_path: toml_source.display().to_string(),
        grid: SweepGrid {
            rounds: dimensions.rounds,
            noise: dimensions.noise,
            repetitions: dimensions.repetitions,
            payoff_preset,
            payoff_r: dimensions.payoff_r,
            payoff_s: dimensions.payoff_s,
            payoff_t: dimensions.payoff_t,
            payoff_p: dimensions.payoff_p,
        },
        cells,
        aggregate: SweepAggregate {
            score_aggregation: scoring_mode,
            adjusted_scores: adjusted,
            strategies,
        },
    }
}

fn persist_summary(
    report: &SweepSummary,
    report_path: &Path,
    format: OutputFormat,
    quiet: bool,
    verbose: bool,
) -> anyhow::Result<()> {
    nit_utils::fs::write_atomic(report_path, |writer| {
        serde_json::to_writer_pretty(writer, report).map_err(std::io::Error::other)
    })
    .with_context(|| format!("failed to write {}", report_path.display()))?;

    if verbose {
        eprintln!("Sweep summary: {}", report_path.display());
    }

    if !quiet {
        let serialized = match format {
            OutputFormat::Json => serde_json::to_string(report)?,
            OutputFormat::Pretty => serde_json::to_string_pretty(report)?,
        };
        println!("{serialized}");
    }

    Ok(())
}
