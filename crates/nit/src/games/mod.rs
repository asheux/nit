//! Games subcommand dispatch: routes CLI commands to tournament execution, parameter sweeps,
//! inspection, graphing, and strategy enumeration handlers.
//!
//! Provides shared infrastructure for configuration loading, path resolution, artifact writing,
//! and tournament execution across sequential and parallel modes.

mod enumerate;
mod inspect;
mod run;
mod sweep;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use anyhow::Context;
use nit_core::io as core_io;
use nit_games::{
    accelerator_run_preflight,
    events::{EventWriter, GameEvent},
    history_log::MatchHistory,
    output::{StrategyDefinition, TournamentResults},
    tournament::{KernelRunMode, Parallelism, TournamentKernel},
    try_select_halting_turing_machine_strategies, GamesConfig, HistoryWriter, NormalizedConfig,
    RuntimeAcceleratorStats, StrategySpec,
};

use crate::cli::GamesCommand;

const DEFAULT_CONFIG_FILENAME: &str = "games.toml";

// ── Subcommand Dispatch ──

/// Route a games subcommand to the appropriate handler.
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
        } => run::run_games_headless(config, strategies, out, seed, format, quiet, verbose),

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
        } => sweep::run_games_sweep(
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
        } => inspect::run_games_inspect(config, id, format, out),

        GamesCommand::Graph {
            config,
            run,
            id,
            out,
        } => inspect::run_games_graph(config, run, id, out),

        GamesCommand::Enumerate { kind } => enumerate::dispatch_enumerate(kind),
    }
}

// ── Template ──

/// Default TOML template for new games workspaces.
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

// ── Config Loading and Path Resolution ──

type LoadedConfig = (PathBuf, String, NormalizedConfig);

/// Load and parse a games config, optionally appending strategies from an NDJSON sidecar.
fn load_games_config(
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
fn resolve_output_dir(
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

/// Make a parent path absolute, falling back to working_dir if None or relative.
fn absolutize_parent(optional_base: Option<&Path>, fallback_cwd: &Path) -> PathBuf {
    match optional_base {
        Some(absolute_base) if absolute_base.is_absolute() => absolute_base.to_path_buf(),
        Some(relative_base) => fallback_cwd.join(relative_base),
        None => fallback_cwd.to_path_buf(),
    }
}

/// Resolve a potentially relative path against a base directory.
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
fn finalize_config(config: NormalizedConfig) -> anyhow::Result<NormalizedConfig> {
    let config = try_select_halting_turing_machine_strategies(config)
        .map_err(|e| anyhow::anyhow!(e))?;
    accelerator_run_preflight(
        &config,
        config.save_data && config.event_log.enabled,
        config.save_data && config.history.enabled,
        false,
    )
    .map_err(|e| anyhow::anyhow!(e))?;
    Ok(config)
}

// ── Strategy Loading ──

/// Load strategy specs from an NDJSON file and append them to the config.
///
/// Blank lines are silently skipped; parse errors include the source path and line number.
fn append_strategies_from_ndjson(
    target_config: &mut NormalizedConfig,
    sidecar_file: &Path,
) -> anyhow::Result<()> {
    use std::io::BufRead;
    let opened_handle = std::fs::File::open(sidecar_file)
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

/// Parse a single NDJSON line into a strategy spec, returning None for blank lines.
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

// ── Artifact Writing ──

/// Persist run artifacts (config snapshot, strategy definitions, tournament results) to disk.
///
/// Individual write failures are logged as warnings rather than propagated, so that
/// partial artifact output is still available even when one file fails.
fn write_run_artifacts(
    toml_output_path: &Path,
    raw_config_content: &str,
    definitions_output_path: &Path,
    compiled_strategy_list: &[StrategyDefinition],
    results_output_path: &Path,
    match_outcome_data: &TournamentResults,
) {
    // Snapshot the raw TOML configuration for reproducibility.
    persist_artifact(toml_output_path, "config snapshot", |target_path| {
        fs::write(target_path, raw_config_content)?;
        Ok(())
    });

    // Serialize compiled strategy definitions to structured JSON.
    persist_artifact(
        definitions_output_path,
        "strategy definitions",
        |target_path| {
            nit_utils::fs::write_atomic(target_path, |json_writer| {
                serde_json::to_writer_pretty(json_writer, compiled_strategy_list)
                    .map_err(std::io::Error::other)
            })?;
            Ok(())
        },
    );

    // Write tournament results with final rankings and per-strategy scores.
    persist_artifact(results_output_path, "tournament results", |target_path| {
        nit_utils::fs::write_atomic(target_path, |json_writer| {
            serde_json::to_writer_pretty(json_writer, match_outcome_data)
                .map_err(std::io::Error::other)
        })?;
        Ok(())
    });
}

/// Write a single artifact file, logging a warning on failure.
fn persist_artifact(
    file_target: &Path,
    description_tag: &str,
    writer_operation: impl FnOnce(&Path) -> anyhow::Result<()>,
) {
    if let Err(io_failure) = writer_operation(file_target) {
        eprintln!("Warning: failed to write {description_tag}: {io_failure}");
    }
}

// ── Tournament Execution ──

struct TournamentRun {
    results: TournamentResults,
    /// GPU utilization and kernel timing metrics.
    runtime: RuntimeAcceleratorStats,
    event_log_path: Option<String>,
    history_log_path: Option<String>,
}

fn execute_tournament(
    tournament_engine: &TournamentKernel,
    event_output_file: Option<PathBuf>,
    history_output_file: Option<PathBuf>,
) -> anyhow::Result<TournamentRun> {
    let engine_settings = tournament_engine.config();
    let parallelism_mode = Parallelism::from_config(&engine_settings.engine.parallelism);

    if matches!(parallelism_mode, Parallelism::Off) {
        run_sequential(
            tournament_engine,
            engine_settings,
            event_output_file,
            history_output_file,
        )
    } else {
        run_parallel(
            tournament_engine,
            engine_settings,
            parallelism_mode,
            event_output_file,
            history_output_file,
        )
    }
}

fn run_sequential(
    tournament_engine: &TournamentKernel,
    engine_settings: &NormalizedConfig,
    event_file: Option<PathBuf>,
    history_file: Option<PathBuf>,
) -> anyhow::Result<TournamentRun> {
    let mut event_recorder = event_file
        .map(|path| EventWriter::new(path, engine_settings.event_log.include_rounds))
        .transpose()?;

    let mut history_recorder = history_file.map(HistoryWriter::new).transpose()?;

    let (tournament_outcomes, acceleration_metrics) =
        tournament_engine.run_with_runtime(KernelRunMode::Sequential {
            event_writer: event_recorder.as_mut(),
            history_writer: history_recorder.as_mut(),
        });

    let finalized_event_path = finalize_writer(event_recorder, "event log")?;
    let finalized_history_path = finalize_writer(history_recorder, "history log")?;

    Ok(TournamentRun {
        results: tournament_outcomes,
        runtime: acceleration_metrics,
        event_log_path: finalized_event_path,
        history_log_path: finalized_history_path,
    })
}

fn run_parallel(
    tournament_engine: &TournamentKernel,
    engine_settings: &NormalizedConfig,
    thread_strategy: Parallelism,
    event_file: Option<PathBuf>,
    history_file: Option<PathBuf>,
) -> anyhow::Result<TournamentRun> {
    let event_writer = event_file
        .map(|path| EventWriter::new(path, engine_settings.event_log.include_rounds))
        .transpose()?;
    let (event_sender, event_thread) = spawn_writer_thread(event_writer);

    let history_writer = history_file.map(HistoryWriter::new).transpose()?;
    let (history_sender, history_thread) = spawn_writer_thread(history_writer);

    let (tournament_outcomes, acceleration_metrics) =
        tournament_engine.run_with_runtime(KernelRunMode::Parallel {
            parallelism: thread_strategy,
            event_sender: event_sender.clone(),
            include_rounds: engine_settings.event_log.include_rounds,
            history_sender: history_sender.clone(),
        });

    drop(event_sender);
    drop(history_sender);

    let finalized_event_path = collect_worker_result(event_thread, "event log")?;
    let finalized_history_path = collect_worker_result(history_thread, "history log")?;

    Ok(TournamentRun {
        results: tournament_outcomes,
        runtime: acceleration_metrics,
        event_log_path: finalized_event_path,
        history_log_path: finalized_history_path,
    })
}

// ── Writer Lifecycle Helpers ──

fn finalize_writer<W: RecordSink>(
    optional_writer: Option<W>,
    writer_description: &str,
) -> anyhow::Result<Option<String>> {
    let Some(open_writer) = optional_writer else {
        return Ok(None);
    };
    let completed_output_path = open_writer
        .finish()
        .with_context(|| format!("failed to finalize {writer_description}"))?;
    Ok(Some(completed_output_path.to_string_lossy().to_string()))
}

type WriterHandle<T> = (
    Option<mpsc::Sender<T>>,
    Option<thread::JoinHandle<std::io::Result<PathBuf>>>,
);

/// Spawn a background writer thread that drains a channel into the given sink.
fn spawn_writer_thread<W: RecordSink>(writer: Option<W>) -> WriterHandle<W::Record> {
    let Some(sink) = writer else {
        return (None, None);
    };
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut sink = sink;
        for record in rx {
            sink.accept(&record)?;
        }
        sink.finish()
    });
    (Some(tx), Some(handle))
}

fn collect_worker_result(
    thread_handle: Option<thread::JoinHandle<std::io::Result<PathBuf>>>,
    writer_description: &str,
) -> anyhow::Result<Option<String>> {
    let Some(active_handle) = thread_handle else {
        return Ok(None);
    };
    let completed_output_path = active_handle
        .join()
        .map_err(|_| anyhow::anyhow!("{writer_description} worker panicked"))?
        .with_context(|| format!("failed to finalize {writer_description}"))?;
    Ok(Some(completed_output_path.to_string_lossy().to_string()))
}

// ── Writer Trait Abstraction ──

/// Unified interface for event and history writers, enabling generic
/// background threading and sequential finalization.
trait RecordSink: Send + 'static {
    type Record: Send + 'static;
    fn accept(&mut self, record: &Self::Record) -> std::io::Result<()>;
    fn finish(self) -> std::io::Result<PathBuf>;
}

impl RecordSink for EventWriter {
    type Record = GameEvent;
    fn accept(&mut self, record: &GameEvent) -> std::io::Result<()> {
        self.write(record)
    }
    fn finish(self) -> std::io::Result<PathBuf> {
        EventWriter::finish(self)
    }
}

impl RecordSink for HistoryWriter {
    type Record = MatchHistory;
    fn accept(&mut self, record: &MatchHistory) -> std::io::Result<()> {
        self.write(record)
    }
    fn finish(self) -> std::io::Result<PathBuf> {
        HistoryWriter::finish(self)
    }
}
