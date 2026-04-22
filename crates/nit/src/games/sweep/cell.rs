use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use nit_games::config::EngineMode;
use nit_games::output::{RunPaths, RunSummary, RUN_SUMMARY_SCHEMA_VERSION};
use nit_games::tournament::TournamentKernel;
use nit_games::{run_id_from_seed_config, NormalizedConfig, ScoreAggregation};
use nit_utils::hashing::stable_hash_bytes;

use super::grid::{payoff_from_rstp, GridCell};
use super::summary::{SweepCellSummary, SweepTopEntry};

/// Maximum number of top-scoring strategies to include in each cell's podium.
const PODIUM_SIZE: usize = 3;

/// Runtime context shared across all cells during a parameter sweep.
pub(super) struct SweepContext<'a> {
    /// Template configuration cloned and mutated per cell.
    pub base_config: &'a NormalizedConfig,
    /// Raw TOML source text for deterministic run ID derivation.
    pub config_text: &'a str,
    /// Root seed from which per-cell seeds are deterministically derived.
    pub base_seed: u64,
    /// ISO-8601 timestamp shared across all cells in this sweep.
    pub timestamp: &'a str,
    pub cells_root: &'a Path,
    /// Recompute cells even if cached results exist on disk.
    pub force: bool,
    pub verbose: bool,
    /// Scoring mode used when ranking strategies within each cell.
    pub score_aggregation: ScoreAggregation,
    /// Whether complexity-adjusted scores are used for ranking.
    pub adjusted_scores: bool,
}

/// Running accumulators updated as each sweep cell completes.
pub(super) struct SweepAccumulators {
    /// Per-strategy score vectors accumulated across all cells.
    pub scores_by_strategy: HashMap<String, Vec<f64>>,
    /// Per-strategy first-place finish counts.
    pub top_counts: HashMap<String, u32>,
}

impl SweepAccumulators {
    pub(super) fn new() -> Self {
        Self {
            scores_by_strategy: HashMap::new(),
            top_counts: HashMap::new(),
        }
    }
}

/// Per-cell configuration produced by applying grid-point overrides to the base sweep config.
struct CellConfig {
    seed: u64,
    normalized: NormalizedConfig,
    serialized: String,
    content_hash: String,
}

/// Execute a single sweep cell: run tournament, write artifacts, collect results.
pub(super) fn run_sweep_cell(
    sweep_context: &SweepContext<'_>,
    running_totals: &mut SweepAccumulators,
    ordinal: usize,
    grid_point: &GridCell,
) -> anyhow::Result<SweepCellSummary> {
    let cell_cfg = prepare_cell_config(sweep_context, grid_point)?;

    let point_output_dir = cell_output_dir(sweep_context.cells_root, ordinal, grid_point);
    fs::create_dir_all(&point_output_dir)
        .with_context(|| format!("failed to create {}", point_output_dir.display()))?;

    let point_summary_file = point_output_dir.join("run_summary.json");

    if point_summary_file.exists() && !sweep_context.force {
        if let Some(cached) = try_reuse_existing_cell(
            &point_summary_file,
            sweep_context,
            running_totals,
            ordinal,
            grid_point,
        ) {
            return Ok(cached);
        }
    }

    let point_config_file = point_output_dir.join("config.toml");
    let point_definitions_file = point_output_dir.join("definitions.json");
    let point_results_file = point_output_dir.join("results.json");
    let point_events_file = point_output_dir.join("events.ndjson");
    let point_history_file = point_output_dir.join("history.ndjson");
    let point_analysis_dir = point_output_dir.join("analysis");

    let tournament_engine = TournamentKernel::new(cell_cfg.normalized.clone());
    let frozen_config = tournament_engine.config().clone();
    let execution_output = super::execute_tournament(
        &tournament_engine,
        cell_cfg
            .normalized
            .event_log
            .enabled
            .then_some(point_events_file),
        cell_cfg
            .normalized
            .history
            .enabled
            .then_some(point_history_file),
    )?;

    super::write_run_artifacts(
        &point_config_file,
        &cell_cfg.serialized,
        &point_definitions_file,
        tournament_engine.definitions(),
        &point_results_file,
        &execution_output.results,
    );

    // Collect top-k before moving results into the summary to avoid a clone.
    let (winner_id, podium_entries) =
        collect_sweep_results(&execution_output.results, sweep_context, running_totals);

    let cell_run_summary = RunSummary {
        schema_version: RUN_SUMMARY_SCHEMA_VERSION,
        timestamp: sweep_context.timestamp.to_owned(),
        run_id: cell_cfg.content_hash.clone(),
        seed: cell_cfg.seed,
        config_text: cell_cfg.serialized,
        config: frozen_config,
        paths: RunPaths {
            summary: Some(point_summary_file.display().to_string()),
            events: execution_output.event_log_path.clone(),
            history: execution_output.history_log_path.clone(),
            definitions: Some(point_definitions_file.display().to_string()),
            results: Some(point_results_file.display().to_string()),
            config: Some(point_config_file.display().to_string()),
            analysis_dir: Some(point_analysis_dir.display().to_string()),
        },
        strategies: tournament_engine.definitions().to_vec(),
        results: execution_output.results,
        event_log: execution_output.event_log_path,
        history_log: execution_output.history_log_path,
        runtime: execution_output.runtime,
        run_dir: Some(point_output_dir.display().to_string()),
    };

    nit_games::output::write_summary(&point_summary_file, &cell_run_summary)
        .with_context(|| format!("failed to write {}", point_summary_file.display()))?;

    Ok(assemble_cell_summary(
        ordinal,
        grid_point,
        cell_cfg.seed,
        cell_cfg.content_hash,
        point_output_dir.display().to_string(),
        point_summary_file.display().to_string(),
        winner_id,
        podium_entries,
        false,
    ))
}

/// Attempt to reuse a previously computed cell result.
fn try_reuse_existing_cell(
    summary_path: &Path,
    sweep_context: &SweepContext<'_>,
    running_totals: &mut SweepAccumulators,
    ordinal: usize,
    grid_point: &GridCell,
) -> Option<SweepCellSummary> {
    let stored_text = fs::read_to_string(summary_path).ok()?;
    let stored_summary: RunSummary = serde_json::from_str(&stored_text).ok()?;

    let (winning_strategy, ranked_entries) =
        collect_sweep_results(&stored_summary.results, sweep_context, running_totals);

    if sweep_context.verbose {
        eprintln!(
            "Skipping existing cell {} ({}): {}",
            ordinal,
            stored_summary.run_id,
            summary_path.display()
        );
    }

    Some(assemble_cell_summary(
        ordinal,
        grid_point,
        stored_summary.seed,
        stored_summary.run_id.clone(),
        stored_summary.run_dir.clone().unwrap_or_else(|| {
            summary_path
                .parent()
                .unwrap_or(Path::new("."))
                .display()
                .to_string()
        }),
        stored_summary
            .paths
            .summary
            .clone()
            .unwrap_or_else(|| summary_path.display().to_string()),
        winning_strategy,
        ranked_entries,
        true,
    ))
}

/// Update running score/top-count accumulators and return the podium for a single cell.
fn collect_sweep_results(
    tournament_outcome: &nit_games::output::TournamentResults,
    sweep_context: &SweepContext<'_>,
    running_totals: &mut SweepAccumulators,
) -> (String, Vec<SweepTopEntry>) {
    let podium_entries: Vec<SweepTopEntry> = tournament_outcome
        .ranking
        .iter()
        .take(PODIUM_SIZE)
        .map(|ranked_item| SweepTopEntry {
            id: ranked_item.id.clone(),
            score: ranked_item.score(
                sweep_context.score_aggregation,
                sweep_context.adjusted_scores,
            ),
        })
        .collect();
    let winner_id = podium_entries
        .first()
        .map(|ranked_item| ranked_item.id.clone())
        .unwrap_or_else(|| "none".into());
    *running_totals
        .top_counts
        .entry(winner_id.clone())
        .or_default() += 1;
    for contestant in &tournament_outcome.ranking {
        running_totals
            .scores_by_strategy
            .entry(contestant.id.clone())
            .or_default()
            .push(contestant.score(
                sweep_context.score_aggregation,
                sweep_context.adjusted_scores,
            ));
    }
    (winner_id, podium_entries)
}

/// Derive a deterministic per-cell seed, apply grid-point overrides, and validate the config.
fn prepare_cell_config(
    ctx: &SweepContext<'_>,
    grid_point: &GridCell,
) -> anyhow::Result<CellConfig> {
    let noise_fingerprint = grid_point.noise.to_bits();
    let seed = stable_hash_bytes(
        format!(
            "{}:{}:{}:{noise_fingerprint}:{}:{}:{}:{}",
            ctx.base_seed,
            grid_point.rounds,
            grid_point.repetitions,
            grid_point.payoff_r,
            grid_point.payoff_s,
            grid_point.payoff_t,
            grid_point.payoff_p
        )
        .as_bytes(),
    );

    let mut config = ctx.base_config.clone();
    config.rounds = grid_point.rounds;
    config.repetitions = grid_point.repetitions;
    config.noise = grid_point.noise.clamp(0.0, 1.0);
    config.payoff = payoff_from_rstp(
        grid_point.payoff_r,
        grid_point.payoff_s,
        grid_point.payoff_t,
        grid_point.payoff_p,
    );
    config.seed = Some(seed);
    config.engine.mode = EngineMode::Batch;
    config = super::finalize_config(config)?;

    let serialized = toml::to_string(&config).unwrap_or_else(|_| ctx.config_text.to_owned());
    let content_hash = run_id_from_seed_config(seed, &serialized);

    Ok(CellConfig {
        seed,
        normalized: config,
        serialized,
        content_hash,
    })
}

/// Build the on-disk directory path for a sweep cell from its ordinal and grid parameters.
fn cell_output_dir(cells_root: &Path, ordinal: usize, grid_point: &GridCell) -> PathBuf {
    let noise_tag = format!("{:.4}", grid_point.noise).replace('.', "_");
    cells_root.join(format!(
        "{ordinal:04}__r{}__n{noise_tag}__rep{}__R{}__S{}__T{}__P{}",
        grid_point.rounds,
        grid_point.repetitions,
        grid_point.payoff_r,
        grid_point.payoff_s,
        grid_point.payoff_t,
        grid_point.payoff_p
    ))
}

#[allow(clippy::too_many_arguments)]
fn assemble_cell_summary(
    cell_id: usize,
    cell: &GridCell,
    seed: u64,
    run_id: String,
    run_dir: String,
    summary_path: String,
    top_strategy: String,
    top_strategies: Vec<SweepTopEntry>,
    skipped: bool,
) -> SweepCellSummary {
    SweepCellSummary {
        cell_id,
        rounds: cell.rounds,
        noise: cell.noise,
        repetitions: cell.repetitions,
        payoff_r: cell.payoff_r,
        payoff_s: cell.payoff_s,
        payoff_t: cell.payoff_t,
        payoff_p: cell.payoff_p,
        seed,
        run_id,
        run_dir,
        summary_path,
        top_strategy,
        top_strategies,
        skipped,
    }
}
