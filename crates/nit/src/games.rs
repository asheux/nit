use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use anyhow::Context;
use nit_core::io as core_io;
use nit_games::config::EngineMode;
use nit_games::events::{EventWriter, GameEvent};
use nit_games::history_log::MatchHistory;
use nit_games::output::{
    write_summary, RunLayout, RunPaths, RunSummary, RUN_SUMMARY_SCHEMA_VERSION,
};
use nit_games::tournament::{KernelRunMode, Parallelism, TournamentKernel};
use nit_games::{
    accelerator_run_preflight, enumerate_fsms, format_strategy_introspection, introspect_strategy,
    run_id_from_seed_config, try_select_halting_turing_machine_strategies, FsmDefinition,
    GamesConfig, HistoryWriter, InputMode, ScoreAggregation, StrategySpec,
};
use nit_utils::hashing::stable_hash_bytes;
use serde::Serialize;

use crate::cli::{EnumerateCommand, GamesCommand, OutputFormat};
use crate::graph::{build_strategy_graph, render_strategy_graph_dot, write_strategy_graph_json};

/// Load a games config from `config_path` (defaulting to `games.toml`), optionally
/// appending strategies from an NDJSON sidecar file.
fn load_games_config(
    config_path: Option<PathBuf>,
    strategies_path: Option<PathBuf>,
) -> anyhow::Result<(PathBuf, String, nit_games::NormalizedConfig)> {
    let config_path = config_path.unwrap_or_else(|| PathBuf::from("games.toml"));
    let config_text = core_io::load_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let mut config = GamesConfig::from_toml_with_root(&config_text, config_path.parent())
        .map_err(|err| anyhow::anyhow!(err))?;
    if let Some(strategies_path) = strategies_path {
        let resolved = resolve_relative_path(&strategies_path, config_path.parent());
        append_strategies_from_ndjson(&mut config, &resolved)?;
    }
    Ok((config_path, config_text, config))
}

/// Resolve the output base directory from a config file's parent, falling back to cwd.
fn resolve_output_dir(config_path: &Path, out_dir: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let base_dir = config_path
        .parent()
        .map(|p| {
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                cwd.join(p)
            }
        })
        .unwrap_or(cwd);
    let out_dir = out_dir.unwrap_or_else(|| base_dir.clone());
    Ok(if out_dir.is_absolute() {
        out_dir
    } else {
        base_dir.join(out_dir)
    })
}

/// Write run artifacts (config snapshot, definitions, results) to disk.
/// Failures are logged as warnings rather than propagated.
fn write_run_artifacts(
    config_path: &Path,
    config_text: &str,
    definitions_path: &Path,
    definitions: &[nit_games::output::StrategyDefinition],
    results_path: &Path,
    results: &nit_games::output::TournamentResults,
) {
    if let Err(err) = fs::write(config_path, config_text) {
        eprintln!("Warning: failed to write config snapshot: {err}");
    }
    if let Err(err) = nit_utils::fs::write_atomic(definitions_path, |writer| {
        serde_json::to_writer_pretty(writer, definitions).map_err(std::io::Error::other)
    }) {
        eprintln!("Warning: failed to write definitions: {err}");
    }
    if let Err(err) = nit_utils::fs::write_atomic(results_path, |writer| {
        serde_json::to_writer_pretty(writer, results).map_err(std::io::Error::other)
    }) {
        eprintln!("Warning: failed to write results: {err}");
    }
}

pub(crate) fn dispatch_subcommand(cmd: GamesCommand) -> anyhow::Result<()> {
    match cmd {
        GamesCommand::Run {
            config,
            strategies,
            out,
            seed,
            format,
            quiet,
            verbose,
        } => run_games_headless(config, strategies, out, seed, format, quiet, verbose),
        GamesCommand::Sweep {
            config,
            strategies,
            out,
            seed,
            rounds,
            noise,
            repetitions,
            payoff_preset,
            payoff_r,
            payoff_s,
            payoff_t,
            payoff_p,
            force,
            format,
            quiet,
            verbose,
        } => run_games_sweep(
            config,
            strategies,
            out,
            seed,
            rounds,
            noise,
            repetitions,
            payoff_preset,
            payoff_r,
            payoff_s,
            payoff_t,
            payoff_p,
            force,
            format,
            quiet,
            verbose,
        ),
        GamesCommand::Inspect {
            config,
            id,
            format,
            out,
        } => run_games_inspect(config, id, format, out),
        GamesCommand::Graph {
            config,
            run,
            id,
            out,
        } => run_games_graph(config, run, id, out),
        GamesCommand::Enumerate { kind } => dispatch_enumerate(kind),
    }
}

fn dispatch_enumerate(kind: EnumerateCommand) -> anyhow::Result<()> {
    match kind {
        EnumerateCommand::Fsm {
            states,
            out,
            canonical,
            limit,
            input_mode,
        } => run_games_enumerate_fsm(&states, &out, canonical, limit, input_mode),
    }
}

pub(crate) fn games_template() -> &'static str {
    r#"schema_version = 1
game = "ipd"
rounds = 200
repetitions = 1
self_play = true
save_data = true
seed = 12345
noise = 0.0

[payoff]
R = -1
S = -3
T = 0
P = -2

[history]
enabled = true
include_cycle_metadata = false

[engine]
mode = "interactive"
parallelism = "auto"
progress_interval_ms = 80
fast_eval = true
score_aggregation = "mean" # Code-02 semantics: per-round average score; TotalPayoff sums matchup means

[engine.complexity_cost]
enabled = false
tm_step_cost = 0.0
fsm_state_cost = 0.0

[[strategy]]
id = "fsm_tft"
type = "auto"
states = 2
start_state = 1
input_index_base = 1
outputs = ["C", "D"]
transitions = [
  [1, 2],
  [1, 2],
]

[[strategy]]
id = "fsm_index_allc"
type = "fsm"
index = 1
num_states = 1
k = 2

[[strategy]]
id = "ca_rule30"
type = "ca"
n = 30
k = 2
r = 1
t = 3

[[strategy]]
id = "tm_rule_3111"
type = "auto"
states = 2
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 128
rule_code = 3111
"#
}

fn execute_tournament(
    kernel: &TournamentKernel,
    event_path: Option<PathBuf>,
    history_path: Option<PathBuf>,
) -> anyhow::Result<(
    nit_games::output::TournamentResults,
    nit_games::RuntimeAcceleratorStats,
    Option<String>,
    Option<String>,
)> {
    let config = kernel.config();
    let parallelism = Parallelism::from_config(&config.engine.parallelism);
    let event_log_enabled = event_path.is_some();
    let history_log_enabled = history_path.is_some();

    let (results, runtime, event_log, history_log) = if matches!(parallelism, Parallelism::Off) {
        let mut event_writer = if event_log_enabled {
            Some(EventWriter::new(
                event_path.clone().expect("event path"),
                config.event_log.include_rounds,
            )?)
        } else {
            None
        };
        let mut history_writer = if history_log_enabled {
            Some(HistoryWriter::new(
                history_path.clone().expect("history path"),
            )?)
        } else {
            None
        };
        let (results, runtime) = kernel.run_with_runtime(KernelRunMode::Sequential {
            event_writer: event_writer.as_mut(),
            history_writer: history_writer.as_mut(),
        });
        let event_log = match event_writer {
            Some(writer) => Some(
                writer
                    .finish()
                    .with_context(|| "failed to finalize event log")?
                    .to_string_lossy()
                    .to_string(),
            ),
            None => None,
        };
        let history_log = match history_writer {
            Some(writer) => Some(
                writer
                    .finish()
                    .with_context(|| "failed to finalize history log")?
                    .to_string_lossy()
                    .to_string(),
            ),
            None => None,
        };
        (results, runtime, event_log, history_log)
    } else {
        let mut event_sender = None;
        let mut history_sender = None;
        let mut event_handle: Option<thread::JoinHandle<std::io::Result<PathBuf>>> = None;
        let mut history_handle: Option<thread::JoinHandle<std::io::Result<PathBuf>>> = None;

        if event_log_enabled {
            let writer = EventWriter::new(
                event_path.clone().expect("event path"),
                config.event_log.include_rounds,
            )?;
            let (tx, rx) = mpsc::channel::<GameEvent>();
            let handle = thread::spawn(move || {
                let mut writer = writer;
                for event in rx {
                    writer.write(&event)?;
                }
                writer.finish()
            });
            event_sender = Some(tx);
            event_handle = Some(handle);
        }

        if history_log_enabled {
            let writer = HistoryWriter::new(history_path.clone().expect("history path"))?;
            let (tx, rx) = mpsc::channel::<MatchHistory>();
            let handle = thread::spawn(move || {
                let mut writer = writer;
                for record in rx {
                    writer.write(&record)?;
                }
                writer.finish()
            });
            history_sender = Some(tx);
            history_handle = Some(handle);
        }

        let (results, runtime) = kernel.run_with_runtime(KernelRunMode::Parallel {
            parallelism,
            event_sender: event_sender.clone(),
            include_rounds: config.event_log.include_rounds,
            history_sender: history_sender.clone(),
        });

        drop(event_sender);
        drop(history_sender);

        let event_log = match event_handle {
            Some(handle) => Some(
                handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("event log worker panicked"))?
                    .with_context(|| "failed to finalize event log")?
                    .to_string_lossy()
                    .to_string(),
            ),
            None => None,
        };
        let history_log = match history_handle {
            Some(handle) => Some(
                handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("history log worker panicked"))?
                    .with_context(|| "failed to finalize history log")?
                    .to_string_lossy()
                    .to_string(),
            ),
            None => None,
        };
        (results, runtime, event_log, history_log)
    };

    Ok((results, runtime, event_log, history_log))
}

fn resolve_relative_path(path: &Path, base_dir: Option<&Path>) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if let Some(base) = base_dir {
        return base.join(path);
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(path)
}

fn append_strategies_from_ndjson(
    config: &mut nit_games::NormalizedConfig,
    path: &Path,
) -> anyhow::Result<()> {
    use std::io::BufRead;
    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open strategies {}", path.display()))?;
    let reader = std::io::BufReader::new(file);
    for (line_idx, line) in reader.lines().enumerate() {
        let line = line.with_context(|| {
            format!(
                "failed to read strategies {} line {}",
                path.display(),
                line_idx + 1
            )
        })?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let spec: StrategySpec = serde_json::from_str(trimmed).with_context(|| {
            format!(
                "failed to parse strategies {} line {}",
                path.display(),
                line_idx + 1
            )
        })?;
        config.strategies.push(spec);
    }
    Ok(())
}

fn run_games_headless(
    config_path: Option<PathBuf>,
    strategies_path: Option<PathBuf>,
    out_dir: Option<PathBuf>,
    seed_override: Option<u64>,
    format: OutputFormat,
    quiet: bool,
    verbose: bool,
) -> anyhow::Result<()> {
    let (config_path, config_text, mut config) = load_games_config(config_path, strategies_path)?;

    if !config.save_data {
        anyhow::bail!("`save_data = false` is not supported for `games run`.");
    }

    if let Some(seed) = seed_override {
        config.seed = Some(seed);
    }
    config.engine.mode = EngineMode::Batch;

    let timestamp = EventWriter::timestamp();
    let seed = config
        .seed
        .unwrap_or_else(|| stable_hash_bytes(format!("{timestamp}\n{config_text}").as_bytes()));
    config.seed = Some(seed);
    config =
        try_select_halting_turing_machine_strategies(config).map_err(|err| anyhow::anyhow!(err))?;
    accelerator_run_preflight(
        &config,
        config.save_data && config.event_log.enabled,
        config.save_data && config.history.enabled,
        false,
    )
    .map_err(|err| anyhow::anyhow!(err))?;

    let run_id = run_id_from_seed_config(seed, &config_text);
    let out_dir = resolve_output_dir(&config_path, out_dir)?;

    let layout = config
        .save_data
        .then(|| RunLayout::for_base(&out_dir, &timestamp, seed, &run_id));
    if let Some(layout) = layout.as_ref() {
        fs::create_dir_all(&layout.run_dir)
            .with_context(|| format!("failed to create {}", layout.run_dir.display()))?;
    }

    let summary_path = layout.as_ref().map(|layout| layout.summary_path.clone());
    let event_path = layout.as_ref().map(|layout| layout.events_path.clone());
    let history_path = layout.as_ref().map(|layout| layout.history_path.clone());

    if verbose {
        eprintln!("Games config: {}", config_path.display());
        match summary_path.as_ref() {
            Some(path) => eprintln!("Games summary: {}", path.display()),
            None => eprintln!("Games summary: disabled (`save_data = false`)"),
        }
    }

    let kernel = TournamentKernel::new(config.clone());
    let event_log_enabled = config.save_data && config.event_log.enabled;
    let history_log_enabled = config.save_data && config.history.enabled;
    let effective_config = kernel.config().clone();
    let (results, runtime, event_log, history_log) = execute_tournament(
        &kernel,
        event_log_enabled.then(|| event_path.clone()).flatten(),
        history_log_enabled.then(|| history_path.clone()).flatten(),
    )?;

    if let Some(layout) = layout.as_ref() {
        write_run_artifacts(
            &layout.config_path,
            &config_text,
            &layout.definitions_path,
            kernel.definitions(),
            &layout.results_path,
            &results,
        );
    }

    let summary = RunSummary {
        schema_version: RUN_SUMMARY_SCHEMA_VERSION,
        timestamp,
        run_id,
        seed,
        config_text: config_text.clone(),
        config: effective_config.clone(),
        paths: RunPaths {
            summary: summary_path.as_ref().map(|path| path.display().to_string()),
            events: event_log.clone(),
            history: history_log.clone(),
            definitions: layout
                .as_ref()
                .map(|layout| layout.definitions_path.display().to_string()),
            results: layout
                .as_ref()
                .map(|layout| layout.results_path.display().to_string()),
            config: layout
                .as_ref()
                .map(|layout| layout.config_path.display().to_string()),
            analysis_dir: layout
                .as_ref()
                .map(|layout| layout.analysis_dir.display().to_string()),
        },
        strategies: kernel.definitions().to_vec(),
        results,
        event_log,
        history_log,
        runtime,
        run_dir: layout
            .as_ref()
            .map(|layout| layout.run_dir.display().to_string()),
    };

    if let Some(summary_path) = summary_path.as_ref() {
        write_summary(summary_path, &summary).with_context(|| {
            let summary_path_display = summary_path.display().to_string();
            format!("failed to write {summary_path_display}")
        })?;
    }

    if verbose {
        if let Some(path) = summary.paths.events.as_ref() {
            eprintln!("Events: {path}");
        }
        if let Some(path) = summary.paths.history.as_ref() {
            eprintln!("History: {path}");
        }
    }

    if !quiet {
        let out = match format {
            OutputFormat::Json => serde_json::to_string(&summary)?,
            OutputFormat::Pretty => serde_json::to_string_pretty(&summary)?,
        };
        println!("{out}");
    }

    Ok(())
}

/// Collect top-k results from a tournament and update the running score/top-count accumulators.
fn collect_sweep_results(
    results: &nit_games::output::TournamentResults,
    score_aggregation: ScoreAggregation,
    adjusted_scores: bool,
    scores_by_strategy: &mut HashMap<String, Vec<f64>>,
    top_counts: &mut HashMap<String, u32>,
) -> (String, Vec<SweepTopEntry>) {
    let top_entries: Vec<SweepTopEntry> = results
        .ranking
        .iter()
        .take(3)
        .map(|entry| SweepTopEntry {
            id: entry.id.clone(),
            score: entry.score(score_aggregation, adjusted_scores),
        })
        .collect();
    let top_id = top_entries
        .first()
        .map(|entry| entry.id.clone())
        .unwrap_or_else(|| "none".into());
    *top_counts.entry(top_id.clone()).or_insert(0) += 1;
    for strategy in &results.ranking {
        scores_by_strategy
            .entry(strategy.id.clone())
            .or_default()
            .push(strategy.score(score_aggregation, adjusted_scores));
    }
    (top_id, top_entries)
}

/// Execute a single sweep cell: build config, run tournament, write artifacts, collect results.
#[allow(clippy::too_many_arguments)]
fn run_sweep_cell(
    base_config: &nit_games::NormalizedConfig,
    config_text_fallback: &str,
    base_seed: u64,
    timestamp: &str,
    cells_root: &Path,
    force: bool,
    verbose: bool,
    score_aggregation: ScoreAggregation,
    adjusted_scores: bool,
    scores_by_strategy: &mut HashMap<String, Vec<f64>>,
    top_counts: &mut HashMap<String, u32>,
    cell_id: usize,
    rounds: u32,
    noise: f32,
    reps: u32,
    r: i32,
    s: i32,
    t: i32,
    p: i32,
) -> anyhow::Result<SweepCellSummary> {
    let noise_bits = noise.to_bits();
    let cell_seed = stable_hash_bytes(
        format!("{base_seed}:{rounds}:{reps}:{noise_bits}:{r}:{s}:{t}:{p}").as_bytes(),
    );

    let mut cell_config = base_config.clone();
    cell_config.rounds = rounds;
    cell_config.repetitions = reps;
    cell_config.noise = noise.clamp(0.0, 1.0);
    cell_config.payoff = payoff_from_rsp(r, s, t, p);
    cell_config.seed = Some(cell_seed);
    cell_config.engine.mode = EngineMode::Batch;
    cell_config = try_select_halting_turing_machine_strategies(cell_config)
        .map_err(|err| anyhow::anyhow!(err))?;
    accelerator_run_preflight(
        &cell_config,
        cell_config.save_data && cell_config.event_log.enabled,
        cell_config.save_data && cell_config.history.enabled,
        false,
    )
    .map_err(|err| anyhow::anyhow!(err))?;

    let config_text_cell =
        toml::to_string(&cell_config).unwrap_or_else(|_| config_text_fallback.to_owned());
    let run_id = run_id_from_seed_config(cell_seed, &config_text_cell);
    let noise_label = format!("{noise:.4}").replace('.', "_");
    let cell_dir = cells_root.join(format!(
        "{cell_id:04}__r{rounds}__n{noise_label}__rep{reps}__R{r}__S{s}__T{t}__P{p}"
    ));
    fs::create_dir_all(&cell_dir)
        .with_context(|| format!("failed to create {}", cell_dir.display()))?;

    let summary_path = cell_dir.join("run_summary.json");

    // Try to reuse an existing cell result if not forcing a rerun.
    if summary_path.exists() && !force {
        if let Some(summary) = fs::read_to_string(&summary_path)
            .ok()
            .and_then(|text| serde_json::from_str::<RunSummary>(&text).ok())
        {
            let (top_id, top_entries) = collect_sweep_results(
                &summary.results,
                score_aggregation,
                adjusted_scores,
                scores_by_strategy,
                top_counts,
            );
            if verbose {
                eprintln!(
                    "Skipping existing cell {} ({}): {}",
                    cell_id,
                    summary.run_id,
                    summary_path.display()
                );
            }
            return Ok(SweepCellSummary {
                cell_id,
                rounds,
                noise,
                repetitions: reps,
                payoff_r: r,
                payoff_s: s,
                payoff_t: t,
                payoff_p: p,
                seed: summary.seed,
                run_id: summary.run_id.clone(),
                run_dir: summary
                    .run_dir
                    .clone()
                    .unwrap_or_else(|| cell_dir.display().to_string()),
                summary_path: summary
                    .paths
                    .summary
                    .clone()
                    .unwrap_or_else(|| summary_path.display().to_string()),
                top_strategy: top_id,
                top_strategies: top_entries,
                skipped: true,
            });
        }
    }

    let config_path = cell_dir.join("config.toml");
    let definitions_path = cell_dir.join("definitions.json");
    let results_path = cell_dir.join("results.json");
    let events_path = cell_dir.join("events.ndjson");
    let history_path = cell_dir.join("history.ndjson");
    let analysis_dir = cell_dir.join("analysis");

    let kernel = TournamentKernel::new(cell_config.clone());
    let effective_cell_config = kernel.config().clone();
    let (results, runtime, event_log, history_log) = execute_tournament(
        &kernel,
        cell_config.event_log.enabled.then_some(events_path),
        cell_config.history.enabled.then_some(history_path),
    )?;

    write_run_artifacts(
        &config_path,
        &config_text_cell,
        &definitions_path,
        kernel.definitions(),
        &results_path,
        &results,
    );

    let summary = RunSummary {
        schema_version: RUN_SUMMARY_SCHEMA_VERSION,
        timestamp: timestamp.to_owned(),
        run_id: run_id.clone(),
        seed: cell_seed,
        config_text: config_text_cell,
        config: effective_cell_config,
        paths: RunPaths {
            summary: Some(summary_path.display().to_string()),
            events: event_log.clone(),
            history: history_log.clone(),
            definitions: Some(definitions_path.display().to_string()),
            results: Some(results_path.display().to_string()),
            config: Some(config_path.display().to_string()),
            analysis_dir: Some(analysis_dir.display().to_string()),
        },
        strategies: kernel.definitions().to_vec(),
        results: results.clone(),
        event_log,
        history_log,
        runtime,
        run_dir: Some(cell_dir.display().to_string()),
    };

    write_summary(&summary_path, &summary)
        .with_context(|| format!("failed to write {}", summary_path.display()))?;

    let (top_id, top_entries) = collect_sweep_results(
        &results,
        score_aggregation,
        adjusted_scores,
        scores_by_strategy,
        top_counts,
    );

    Ok(SweepCellSummary {
        cell_id,
        rounds,
        noise,
        repetitions: reps,
        payoff_r: r,
        payoff_s: s,
        payoff_t: t,
        payoff_p: p,
        seed: cell_seed,
        run_id,
        run_dir: cell_dir.display().to_string(),
        summary_path: summary_path.display().to_string(),
        top_strategy: top_id,
        top_strategies: top_entries,
        skipped: false,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_games_sweep(
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
    let (config_path, config_text, mut config) = load_games_config(config_path, strategies_path)?;

    let timestamp = EventWriter::timestamp();
    let base_seed = seed_override
        .or(config.seed)
        .unwrap_or_else(|| stable_hash_bytes(format!("{timestamp}\n{config_text}").as_bytes()));
    config.seed = Some(base_seed);
    config.engine.mode = EngineMode::Batch;

    fn grid_or_default<T>(grid: Vec<T>, fallback: T) -> Vec<T> {
        if grid.is_empty() {
            vec![fallback]
        } else {
            grid
        }
    }
    let rounds_grid = grid_or_default(rounds, config.rounds);
    let noise_grid = grid_or_default(noise, config.noise);
    let reps_grid = grid_or_default(repetitions, config.repetitions);

    let (base_r, base_s, base_t, base_p) = match payoff_preset.as_deref() {
        Some(name) => resolve_payoff_preset(name)
            .ok_or_else(|| anyhow::anyhow!("unknown payoff preset '{name}'"))?,
        None => (
            config.payoff.r,
            config.payoff.s,
            config.payoff.t,
            config.payoff.p,
        ),
    };
    let payoff_r_grid = grid_or_default(payoff_r, base_r);
    let payoff_s_grid = grid_or_default(payoff_s, base_s);
    let payoff_t_grid = grid_or_default(payoff_t, base_t);
    let payoff_p_grid = grid_or_default(payoff_p, base_p);

    let out_dir = resolve_output_dir(&config_path, out_dir)?;

    let stamp = timestamp.replace(':', "-");
    let sweep_root = out_dir
        .join("runs")
        .join("games")
        .join("sweeps")
        .join(format!("{stamp}__seed-{base_seed}"));
    let cells_root = sweep_root.join("cells");
    fs::create_dir_all(&cells_root)
        .with_context(|| format!("failed to create {}", cells_root.display()))?;

    let mut cell_summaries = Vec::new();
    let score_aggregation = config.engine.score_aggregation;
    let adjusted_scores = config.engine.complexity_cost.enabled;
    let mut scores_by_strategy: HashMap<String, Vec<f64>> = HashMap::new();
    let mut top_counts: HashMap<String, u32> = HashMap::new();

    // Build the full Cartesian product as a flat list to avoid 7-level nesting.
    let grid_cells: Vec<_> = {
        let mut cells = Vec::new();
        for &rounds in &rounds_grid {
            for &noise in &noise_grid {
                for &reps in &reps_grid {
                    for &r in &payoff_r_grid {
                        for &s in &payoff_s_grid {
                            for &t in &payoff_t_grid {
                                for &p in &payoff_p_grid {
                                    cells.push((rounds, noise, reps, r, s, t, p));
                                }
                            }
                        }
                    }
                }
            }
        }
        cells
    };

    for (cell_idx, &(rounds, noise, reps, r, s, t, p)) in grid_cells.iter().enumerate() {
        let cell_id = cell_idx + 1;
        let summary = run_sweep_cell(
            &config,
            &config_text,
            base_seed,
            &timestamp,
            &cells_root,
            force,
            verbose,
            score_aggregation,
            adjusted_scores,
            &mut scores_by_strategy,
            &mut top_counts,
            cell_id,
            rounds,
            noise,
            reps,
            r,
            s,
            t,
            p,
        )?;
        cell_summaries.push(summary);
    }

    let mut strategies = Vec::new();
    for (id, scores) in scores_by_strategy {
        let count = scores.len() as f64;
        let mean = scores.iter().sum::<f64>() / count.max(1.0);
        let var = scores
            .iter()
            .map(|v| {
                let diff = *v - mean;
                diff * diff
            })
            .sum::<f64>()
            / count.max(1.0);
        let top_count = top_counts.get(&id).copied().unwrap_or(0);
        strategies.push(SweepStrategyAggregate {
            id,
            mean_score: mean,
            std_score: var.sqrt(),
            top1_count: top_count,
        });
    }
    strategies.sort_by(|a, b| b.mean_score.partial_cmp(&a.mean_score).unwrap());

    let summary = SweepSummary {
        schema_version: 1,
        timestamp: timestamp.clone(),
        seed: base_seed,
        config_path: config_path.display().to_string(),
        grid: SweepGrid {
            rounds: rounds_grid.clone(),
            noise: noise_grid.clone(),
            repetitions: reps_grid.clone(),
            payoff_preset: payoff_preset.clone(),
            payoff_r: payoff_r_grid.clone(),
            payoff_s: payoff_s_grid.clone(),
            payoff_t: payoff_t_grid.clone(),
            payoff_p: payoff_p_grid.clone(),
        },
        cells: cell_summaries,
        aggregate: SweepAggregate {
            score_aggregation,
            adjusted_scores,
            strategies,
        },
    };

    let summary_path = sweep_root.join("sweep_summary.json");
    nit_utils::fs::write_atomic(&summary_path, |writer| {
        serde_json::to_writer_pretty(writer, &summary).map_err(std::io::Error::other)
    })
    .with_context(|| format!("failed to write {}", summary_path.display()))?;

    if verbose {
        eprintln!("Sweep summary: {}", summary_path.display());
    }

    if !quiet {
        let out = match format {
            OutputFormat::Json => serde_json::to_string(&summary)?,
            OutputFormat::Pretty => serde_json::to_string_pretty(&summary)?,
        };
        println!("{out}");
    }

    Ok(())
}

fn run_games_inspect(
    config_path: Option<PathBuf>,
    id: String,
    format: OutputFormat,
    out: Option<PathBuf>,
) -> anyhow::Result<()> {
    let (_config_path, _config_text, config) = load_games_config(config_path, None)?;

    let spec = config
        .strategies
        .iter()
        .find(|spec| spec.id == id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("strategy '{id}' not found"))?;
    let intro = introspect_strategy(&spec);
    let output = match format {
        OutputFormat::Json => serde_json::to_string(&intro)?,
        OutputFormat::Pretty => format_strategy_introspection(&intro).join("\n"),
    };

    if let Some(out_path) = out {
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                let parent_display = parent.display().to_string();
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create directory {parent_display}"))?;
            }
        }
        let out_path_display = out_path.display().to_string();
        fs::write(&out_path, output)
            .with_context(|| format!("failed to write {out_path_display}"))?;
    } else {
        println!("{output}");
    }

    Ok(())
}

fn run_games_graph(
    config_path: Option<PathBuf>,
    run_path: Option<PathBuf>,
    strategy_id: String,
    out_path: PathBuf,
) -> anyhow::Result<()> {
    let spec = if let Some(run_path) = run_path {
        let run_path_display = run_path.display().to_string();
        let run_text = core_io::load_to_string(&run_path)
            .with_context(|| format!("failed to read {run_path_display}"))?;
        let summary: RunSummary = serde_json::from_str(&run_text)
            .with_context(|| format!("failed to parse {run_path_display}"))?;
        summary
            .config
            .strategies
            .iter()
            .find(|spec| spec.id == strategy_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("strategy '{strategy_id}' not found"))?
    } else {
        let (_config_path, _config_text, config) = load_games_config(config_path, None)?;
        config
            .strategies
            .iter()
            .find(|spec| spec.id == strategy_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("strategy '{strategy_id}' not found"))?
    };
    let intro = introspect_strategy(&spec);
    let graph = build_strategy_graph(&intro)?;

    let ext = out_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");
    let is_json = ext.eq_ignore_ascii_case("json");
    let is_dot = ext.eq_ignore_ascii_case("dot") || ext.eq_ignore_ascii_case("gv");
    if !is_json && !is_dot {
        anyhow::bail!("output path must end with .json, .dot, or .gv");
    }

    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }
    }

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

fn run_games_enumerate_fsm(
    states: &str,
    out: &Path,
    canonical: bool,
    limit: Option<usize>,
    input_mode: Option<String>,
) -> anyhow::Result<()> {
    let range = parse_states_range(states)?;
    let mode = parse_input_mode_arg(input_mode.as_deref())?;

    let out_path = if out.extension().map(|ext| ext == "ndjson").unwrap_or(false) {
        out.to_path_buf()
    } else {
        fs::create_dir_all(out)?;
        let filename = format!(
            "fsm_enumeration__states-{}.ndjson",
            states.replace("..", "-")
        );
        out.join(filename)
    };

    let mut writer = std::io::BufWriter::new(
        std::fs::File::create(&out_path)
            .with_context(|| format!("failed to create {}", out_path.display()))?,
    );

    let mut total = 0usize;
    for states in range {
        let remaining = limit.and_then(|limit| limit.checked_sub(total));
        if matches!(remaining, Some(0)) {
            break;
        }
        total += enumerate_fsms(states, mode, remaining, canonical, |def: FsmDefinition| {
            let id = format!("fsm_{:016x}", def.stable_hash());
            let spec = def.to_spec(id);
            serde_json::to_writer(&mut writer, &spec).expect("write fsm strategy");
            writer.write_all(b"\n").expect("write newline");
        });
    }

    writer.flush()?;
    eprintln!("FSM enumeration written: {}", out_path.display());
    Ok(())
}

fn parse_states_range(input: &str) -> anyhow::Result<std::ops::RangeInclusive<usize>> {
    let trimmed = input.trim();
    if let Some((left, right)) = trimmed.split_once("..=") {
        let start: usize = left.trim().parse()?;
        let end: usize = right.trim().parse()?;
        if start > end {
            anyhow::bail!("states range start must be <= end");
        }
        return Ok(start..=end);
    }
    if let Some((left, right)) = trimmed.split_once("..") {
        let start: usize = left.trim().parse()?;
        let end: usize = right.trim().parse()?;
        if start > end {
            anyhow::bail!("states range start must be <= end");
        }
        return Ok(start..=end);
    }
    let value: usize = trimmed.parse()?;
    Ok(value..=value)
}

fn parse_input_mode_arg(input: Option<&str>) -> anyhow::Result<InputMode> {
    let Some(raw) = input else {
        return Ok(InputMode::OpponentLastAction);
    };
    let normalized: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let mode = match normalized.as_str() {
        "opponentlastaction" | "opponent" | "opp" | "opplastaction" => {
            InputMode::OpponentLastAction
        }
        "selflastaction" | "self" | "selflast" => InputMode::SelfLastAction,
        "jointlastaction" | "joint" | "jointlast" => InputMode::JointLastAction,
        _ => anyhow::bail!(
            "invalid input_mode '{raw}': expected opponent_last_action, self_last_action, or joint_last_action"
        ),
    };
    Ok(mode)
}

fn resolve_payoff_preset(name: &str) -> Option<(i32, i32, i32, i32)> {
    let normalized: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    match normalized.as_str() {
        "pd" | "prisonersdilemma" | "prisoner" => Some((3, 0, 5, 1)),
        "staghunt" | "stag" => Some((4, 1, 3, 2)),
        "snowdrift" | "snow" | "hawkedove" | "hawkdove" => Some((3, 1, 5, 0)),
        "chicken" => Some((3, 1, 5, 0)),
        _ => None,
    }
}

fn payoff_from_rsp(r: i32, s: i32, t: i32, p: i32) -> nit_games::PayoffMatrix {
    nit_games::PayoffMatrix::from_matrix([[[r, r], [s, t]], [[t, s], [p, p]]])
}

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

#[derive(Serialize)]
struct SweepAggregate {
    score_aggregation: ScoreAggregation,
    adjusted_scores: bool,
    strategies: Vec<SweepStrategyAggregate>,
}

#[derive(Serialize)]
struct SweepStrategyAggregate {
    id: String,
    mean_score: f64,
    std_score: f64,
    top1_count: u32,
}

#[derive(Serialize)]
struct SweepTopEntry {
    id: String,
    score: f64,
}
