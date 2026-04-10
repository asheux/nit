//! Parameter sweep runner: executes tournaments across a Cartesian grid of game parameters.
//!
//! Each grid cell is a complete IPD tournament with its own seed, config, and output directory.
//! Results are aggregated per-strategy with mean scores, standard deviation, and top-1 counts.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use nit_games::config::EngineMode;
use nit_games::events::EventWriter;
use nit_games::output::{RunPaths, RunSummary, RUN_SUMMARY_SCHEMA_VERSION};
use nit_games::tournament::TournamentKernel;
use nit_games::{run_id_from_seed_config, NormalizedConfig, PayoffMatrix, ScoreAggregation};
use nit_utils::hashing::stable_hash_bytes;
use serde::Serialize;

use crate::cli::OutputFormat;

/// Maximum number of top-scoring strategies to include in each cell's podium.
const PODIUM_SIZE: usize = 3;

/// Schema version for sweep summary output files.
const SWEEP_SCHEMA_VERSION: u32 = 1;

/// Runtime context shared across all cells during a parameter sweep.
struct SweepContext<'a> {
    /// Template configuration cloned and mutated per cell.
    base_config: &'a nit_games::NormalizedConfig,
    /// Raw TOML source text for deterministic run ID derivation.
    config_text: &'a str,
    /// Root seed from which per-cell seeds are deterministically derived.
    base_seed: u64,
    /// ISO-8601 timestamp shared across all cells in this sweep.
    timestamp: &'a str,
    cells_root: &'a Path,
    /// Recompute cells even if cached results exist on disk.
    force: bool,
    verbose: bool,
    /// Scoring mode used when ranking strategies within each cell.
    score_aggregation: ScoreAggregation,
    /// Whether complexity-adjusted scores are used for ranking.
    adjusted_scores: bool,
}

/// Running accumulators updated as each sweep cell completes.
struct SweepAccumulators {
    /// Per-strategy score vectors accumulated across all cells.
    scores_by_strategy: HashMap<String, Vec<f64>>,
    /// Per-strategy first-place finish counts.
    top_counts: HashMap<String, u32>,
}

/// Per-cell configuration produced by applying grid-point overrides to the base sweep config.
struct CellConfig {
    seed: u64,
    normalized: NormalizedConfig,
    serialized: String,
    content_hash: String,
}

struct GridCell {
    rounds: u32,
    /// Bit-flip probability per round.
    noise: f32,
    repetitions: u32,
    payoff_r: i32,
    payoff_s: i32,
    payoff_t: i32,
    payoff_p: i32,
}

struct ParameterGrids {
    rounds: Vec<u32>,
    noise: Vec<f32>,
    repetitions: Vec<u32>,
    payoff_r: Vec<i32>,
    payoff_s: Vec<i32>,
    payoff_t: Vec<i32>,
    payoff_p: Vec<i32>,
}

/// Resolve a parameter dimension: CLI overrides take precedence,
/// falling back to the single value from the parsed config.
fn grid_or_default<T>(explicit_values: Vec<T>, config_fallback: T) -> Vec<T> {
    if explicit_values.is_empty() {
        vec![config_fallback]
    } else {
        explicit_values
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_games_sweep(
    config_path: Option<PathBuf>,
    strategies_path: Option<PathBuf>,
    out_dir: Option<PathBuf>,
    seed_override: Option<u64>,
    rounds: Vec<u32>,
    noise: Vec<f32>,
    repetitions: Vec<u32>,
    payoff_preset: Option<String>,
    payoff_r: Vec<i32>,
    payoff_s: Vec<i32>,
    payoff_t: Vec<i32>,
    payoff_p: Vec<i32>,
    force: bool,
    format: OutputFormat,
    quiet: bool,
    verbose: bool,
) -> anyhow::Result<()> {
    let (toml_source, raw_toml, mut parsed_config) =
        super::load_games_config(config_path, strategies_path)?;

    let sweep_timestamp = EventWriter::timestamp();
    let root_seed = seed_override
        .or(parsed_config.seed)
        .unwrap_or_else(|| stable_hash_bytes(format!("{sweep_timestamp}\n{raw_toml}").as_bytes()));
    parsed_config.seed = Some(root_seed);
    parsed_config.engine.mode = EngineMode::Batch;

    let parameter_dimensions = resolve_parameter_grids(
        &parsed_config,
        rounds,
        noise,
        repetitions,
        payoff_preset.as_deref(),
        payoff_r,
        payoff_s,
        payoff_t,
        payoff_p,
    )?;

    let output_root = super::resolve_output_dir(&toml_source, out_dir)?;

    let sanitized_timestamp = sweep_timestamp.replace(':', "-");
    let sweep_output_dir = output_root
        .join("runs")
        .join("games")
        .join("sweeps")
        .join(format!("{sanitized_timestamp}__seed-{root_seed}"));
    let cell_storage_dir = sweep_output_dir.join("cells");
    fs::create_dir_all(&cell_storage_dir)
        .with_context(|| format!("failed to create {}", cell_storage_dir.display()))?;

    let sweep_context = SweepContext {
        base_config: &parsed_config,
        config_text: &raw_toml,
        base_seed: root_seed,
        timestamp: &sweep_timestamp,
        cells_root: &cell_storage_dir,
        force,
        verbose,
        score_aggregation: parsed_config.engine.score_aggregation,
        adjusted_scores: parsed_config.engine.complexity_cost.enabled,
    };
    let mut running_totals = SweepAccumulators {
        scores_by_strategy: HashMap::new(),
        top_counts: HashMap::new(),
    };

    let cartesian_cells = build_cartesian_grid(&parameter_dimensions);

    let mut completed_cells = Vec::new();
    for (ordinal, grid_point) in cartesian_cells.iter().enumerate() {
        let cell_result =
            run_sweep_cell(&sweep_context, &mut running_totals, ordinal + 1, grid_point)?;
        completed_cells.push(cell_result);
    }

    let aggregated_rankings = compute_sweep_aggregates(running_totals);

    let sweep_report = SweepSummary {
        schema_version: SWEEP_SCHEMA_VERSION,
        timestamp: sweep_timestamp.clone(),
        seed: root_seed,
        config_path: toml_source.display().to_string(),
        grid: SweepGrid {
            rounds: parameter_dimensions.rounds,
            noise: parameter_dimensions.noise,
            repetitions: parameter_dimensions.repetitions,
            payoff_preset: payoff_preset.clone(),
            payoff_r: parameter_dimensions.payoff_r,
            payoff_s: parameter_dimensions.payoff_s,
            payoff_t: parameter_dimensions.payoff_t,
            payoff_p: parameter_dimensions.payoff_p,
        },
        cells: completed_cells,
        aggregate: SweepAggregate {
            score_aggregation: sweep_context.score_aggregation,
            adjusted_scores: sweep_context.adjusted_scores,
            strategies: aggregated_rankings,
        },
    };

    let report_output_path = sweep_output_dir.join("sweep_summary.json");
    nit_utils::fs::write_atomic(&report_output_path, |writer| {
        serde_json::to_writer_pretty(writer, &sweep_report).map_err(std::io::Error::other)
    })
    .with_context(|| format!("failed to write {}", report_output_path.display()))?;

    if verbose {
        eprintln!("Sweep summary: {}", report_output_path.display());
    }

    if !quiet {
        let formatted_output = match format {
            OutputFormat::Json => serde_json::to_string(&sweep_report)?,
            OutputFormat::Pretty => serde_json::to_string_pretty(&sweep_report)?,
        };
        println!("{formatted_output}");
    }

    Ok(())
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

/// Execute a single sweep cell: run tournament, write artifacts, collect results.
fn run_sweep_cell(
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

/// Attempt to reuse a previously computed cell result, returning the summary if found.
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

/// Construct a cell summary from resolved tournament outputs.
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

/// Known payoff matrix presets from game theory literature.
#[derive(Debug, Clone, Copy)]
enum PayoffPreset {
    /// Classic Prisoner's Dilemma: temptation to defect dominates.
    PrisonersDilemma,
    /// Stag Hunt: coordination game with Pareto-dominant equilibrium.
    StagHunt,
    /// Snowdrift / Hawk-Dove: anti-coordination with mixed equilibrium.
    Snowdrift,
}

impl PayoffPreset {
    fn from_label(preset_label: &str) -> Option<Self> {
        let canonical: String = preset_label
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .map(|ch| ch.to_ascii_lowercase())
            .collect();
        match canonical.as_str() {
            "pd" | "prisonersdilemma" | "prisoner" => Some(Self::PrisonersDilemma),
            "staghunt" | "stag" => Some(Self::StagHunt),
            "snowdrift" | "snow" | "hawkedove" | "hawkdove" | "chicken" => Some(Self::Snowdrift),
            _ => None,
        }
    }

    /// Return the canonical (R, S, T, P) values for this preset.
    const fn payoff_values(self) -> (i32, i32, i32, i32) {
        match self {
            Self::PrisonersDilemma => (3, 0, 5, 1),
            Self::StagHunt => (4, 1, 3, 2),
            Self::Snowdrift => (3, 1, 5, 0),
        }
    }
}

/// Construct a symmetric 2x2 payoff matrix from the four canonical payoff values.
///
/// The matrix encodes: R (reward for mutual cooperation), S (sucker's payoff),
/// T (temptation to defect), and P (punishment for mutual defection).
fn payoff_from_rstp(reward: i32, sucker: i32, temptation: i32, punishment: i32) -> PayoffMatrix {
    PayoffMatrix::from_matrix([
        [[reward, reward], [sucker, temptation]],
        [[temptation, sucker], [punishment, punishment]],
    ])
}

/// Resolve CLI grid vectors against config defaults, applying payoff presets if specified.
#[allow(clippy::too_many_arguments)]
fn resolve_parameter_grids(
    fallback_config: &nit_games::NormalizedConfig,
    explicit_rounds: Vec<u32>,
    explicit_noise: Vec<f32>,
    explicit_repetitions: Vec<u32>,
    named_preset: Option<&str>,
    explicit_reward: Vec<i32>,
    explicit_sucker: Vec<i32>,
    explicit_temptation: Vec<i32>,
    explicit_punishment: Vec<i32>,
) -> anyhow::Result<ParameterGrids> {
    let (fallback_reward, fallback_sucker, fallback_temptation, fallback_punishment) =
        match named_preset {
            Some(preset_key) => PayoffPreset::from_label(preset_key)
                .map(|p| p.payoff_values())
                .ok_or_else(|| anyhow::anyhow!("unknown payoff preset '{preset_key}'"))?,
            None => (
                fallback_config.payoff.r,
                fallback_config.payoff.s,
                fallback_config.payoff.t,
                fallback_config.payoff.p,
            ),
        };

    Ok(ParameterGrids {
        rounds: grid_or_default(explicit_rounds, fallback_config.rounds),
        noise: grid_or_default(explicit_noise, fallback_config.noise),
        repetitions: grid_or_default(explicit_repetitions, fallback_config.repetitions),
        payoff_r: grid_or_default(explicit_reward, fallback_reward),
        payoff_s: grid_or_default(explicit_sucker, fallback_sucker),
        payoff_t: grid_or_default(explicit_temptation, fallback_temptation),
        payoff_p: grid_or_default(explicit_punishment, fallback_punishment),
    })
}

/// Build the full Cartesian product of parameter dimensions as a flat grid.
fn build_cartesian_grid(space: &ParameterGrids) -> Vec<GridCell> {
    let capacity = space.rounds.len()
        * space.noise.len()
        * space.repetitions.len()
        * space.payoff_r.len()
        * space.payoff_s.len()
        * space.payoff_t.len()
        * space.payoff_p.len();

    let mut grid = Vec::with_capacity(capacity);
    for &rounds in &space.rounds {
        for &noise in &space.noise {
            for &repetitions in &space.repetitions {
                expand_payoff_combinations(&mut grid, rounds, noise, repetitions, space);
            }
        }
    }
    grid
}

/// Expand the payoff-matrix dimensions and append GridCells for one execution triple.
fn expand_payoff_combinations(
    output: &mut Vec<GridCell>,
    round_count: u32,
    noise_level: f32,
    rep_count: u32,
    space: &ParameterGrids,
) {
    for &reward_val in &space.payoff_r {
        for &sucker_val in &space.payoff_s {
            for &temptation_val in &space.payoff_t {
                for &punishment_val in &space.payoff_p {
                    output.push(GridCell {
                        rounds: round_count,
                        noise: noise_level,
                        repetitions: rep_count,
                        payoff_r: reward_val,
                        payoff_s: sucker_val,
                        payoff_t: temptation_val,
                        payoff_p: punishment_val,
                    });
                }
            }
        }
    }
}

/// Aggregate per-strategy scores into ranked summary statistics.
fn compute_sweep_aggregates(completed_data: SweepAccumulators) -> Vec<SweepStrategyAggregate> {
    let mut sorted_rankings = Vec::new();
    for (contestant_name, observed_scores) in completed_data.scores_by_strategy {
        let observation_count = observed_scores.len() as f64;
        let arithmetic_mean = observed_scores.iter().sum::<f64>() / observation_count.max(1.0);
        // Compute population variance as mean of squared residuals.
        let variance = observed_scores
            .iter()
            .map(|score| (*score - arithmetic_mean).powi(2))
            .sum::<f64>()
            / observation_count.max(1.0);
        let victory_count = completed_data
            .top_counts
            .get(&contestant_name)
            .copied()
            .unwrap_or(0);
        sorted_rankings.push(SweepStrategyAggregate {
            id: contestant_name,
            mean_score: arithmetic_mean,
            std_score: variance.sqrt(),
            top1_count: victory_count,
        });
    }
    sorted_rankings.sort_by(|a, b| b.mean_score.partial_cmp(&a.mean_score).unwrap());
    sorted_rankings
}

/// Top-level sweep output containing grid configuration, per-cell results, and aggregates.
#[derive(Serialize)]
struct SweepSummary {
    schema_version: u32,
    timestamp: String,
    seed: u64,
    config_path: String,
    grid: SweepGrid,
    cells: Vec<SweepCellSummary>,
    aggregate: SweepAggregate,
}

/// The parameter space axes that define which dimensions are swept.
#[derive(Serialize)]
struct SweepGrid {
    rounds: Vec<u32>,
    noise: Vec<f32>,
    repetitions: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    payoff_preset: Option<String>,
    payoff_r: Vec<i32>,
    payoff_s: Vec<i32>,
    payoff_t: Vec<i32>,
    payoff_p: Vec<i32>,
}

/// Result of running (or reusing) a single grid cell tournament.
#[derive(Serialize)]
struct SweepCellSummary {
    cell_id: usize,
    rounds: u32,
    noise: f32,
    repetitions: u32,
    payoff_r: i32,
    payoff_s: i32,
    payoff_t: i32,
    payoff_p: i32,
    seed: u64,
    run_id: String,
    run_dir: String,
    summary_path: String,
    top_strategy: String,
    top_strategies: Vec<SweepTopEntry>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    skipped: bool,
}

/// Cross-cell aggregate statistics: scoring mode and per-strategy rankings.
#[derive(Serialize)]
struct SweepAggregate {
    score_aggregation: ScoreAggregation,
    adjusted_scores: bool,
    strategies: Vec<SweepStrategyAggregate>,
}

/// Per-strategy aggregate: mean score, standard deviation, and first-place win count.
#[derive(Serialize)]
struct SweepStrategyAggregate {
    id: String,
    mean_score: f64,
    std_score: f64,
    top1_count: u32,
}

/// A ranked entry in a cell's top-k podium.
#[derive(Serialize)]
struct SweepTopEntry {
    id: String,
    score: f64,
}
