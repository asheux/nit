//! Headless tournament runner: single-config batch execution with artifact output.
//!
//! Loads a games configuration, validates it, executes a deterministic tournament,
//! writes output artifacts (config snapshot, definitions, results), and emits a summary.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use nit_games::events::EventWriter;
use nit_games::output::{
    RunLayout, RunPaths, RunSummary, StrategyDefinition, RUN_SUMMARY_SCHEMA_VERSION,
};
use nit_games::tournament::TournamentKernel;
use nit_games::{
    accelerator_run_preflight, config::EngineMode, try_select_halting_turing_machine_strategies,
    NormalizedConfig,
};
use nit_utils::hashing::stable_hash_bytes;

use crate::cli::OutputFormat;

const SAVE_DATA_REQUIRED_MSG: &str = "`save_data = false` is not supported for `games run`.";

/// Phases of a headless tournament run, used for diagnostic tracing.
#[derive(Debug, Clone, Copy)]
enum RunPhase {
    ConfigPrep,
    Execution,
    ArtifactWrite,
    SummaryEmit,
}

impl std::fmt::Display for RunPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConfigPrep => f.write_str("config-prep"),
            Self::Execution => f.write_str("execution"),
            Self::ArtifactWrite => f.write_str("artifact-write"),
            Self::SummaryEmit => f.write_str("summary-emit"),
        }
    }
}

struct PreparedBatchConfig {
    resolved_path: PathBuf,
    source_text: String,
    /// All defaults applied and validated.
    normalized: NormalizedConfig,
    batch_timestamp: String,
    effective_seed: u64,
    /// Content-addressed: derived from seed + config text.
    deterministic_run_id: String,
}

/// Run a single headless tournament, write output artifacts, and emit a summary.
pub(super) fn run_games_headless(
    config_path: Option<PathBuf>,
    strategies_path: Option<PathBuf>,
    out_dir: Option<PathBuf>,
    seed_override: Option<u64>,
    output_format: OutputFormat,
    suppress_stdout: bool,
    verbose_logging: bool,
) -> anyhow::Result<()> {
    // Phase 1: load and validate configuration.
    let batch_cfg = prepare_batch_config(config_path, strategies_path, seed_override)?;
    let artifact_root = super::resolve_output_dir(&batch_cfg.resolved_path, out_dir)?;

    // Phase 2: determine on-disk layout when data saving is enabled.
    let storage_layout = batch_cfg.normalized.save_data.then(|| {
        RunLayout::for_base(
            &artifact_root,
            &batch_cfg.batch_timestamp,
            batch_cfg.effective_seed,
            &batch_cfg.deterministic_run_id,
        )
    });

    if let Some(ref layout_ref) = storage_layout {
        fs::create_dir_all(&layout_ref.run_dir)
            .with_context(|| format!("failed to create {}", layout_ref.run_dir.display()))?;
    }

    if verbose_logging {
        log_run_preamble(&batch_cfg.resolved_path, &storage_layout);
    }

    // Phase 3: build and execute the tournament kernel.
    if verbose_logging {
        eprintln!("[{}] Starting tournament execution", RunPhase::Execution);
    }
    let engine = TournamentKernel::new(batch_cfg.normalized.clone());
    let snapshot_config = engine.config().clone();
    let emit_events = batch_cfg.normalized.save_data && batch_cfg.normalized.event_log.enabled;
    let emit_history = batch_cfg.normalized.save_data && batch_cfg.normalized.history.enabled;

    let execution_output = super::execute_tournament(
        &engine,
        emit_events
            .then(|| storage_layout.as_ref().map(|dl| dl.events_path.clone()))
            .flatten(),
        emit_history
            .then(|| storage_layout.as_ref().map(|dl| dl.history_path.clone()))
            .flatten(),
    )?;

    // Phase 4: write artifacts to the run directory.
    if verbose_logging {
        eprintln!("[{}] Writing run artifacts", RunPhase::ArtifactWrite);
    }
    if let Some(ref artifact_layout) = storage_layout {
        super::write_run_artifacts(
            &artifact_layout.config_path,
            &batch_cfg.source_text,
            &artifact_layout.definitions_path,
            engine.definitions(),
            &artifact_layout.results_path,
            &execution_output.results,
        );
    }

    // Phase 5: assemble and emit the summary.
    let final_summary = build_headless_summary(
        &batch_cfg,
        snapshot_config,
        &storage_layout,
        engine.definitions(),
        execution_output,
    );

    persist_and_emit_summary(
        &final_summary,
        &storage_layout,
        output_format,
        suppress_stdout,
        verbose_logging,
    )
}

fn prepare_batch_config(
    toml_source: Option<PathBuf>,
    sidecar_source: Option<PathBuf>,
    explicit_seed_value: Option<u64>,
) -> anyhow::Result<PreparedBatchConfig> {
    let (canonical_path, raw_toml, mut parsed_cfg) =
        super::load_games_config(toml_source, sidecar_source)?;

    if !parsed_cfg.save_data {
        anyhow::bail!(SAVE_DATA_REQUIRED_MSG);
    }

    // Apply explicit seed if provided; force batch engine mode.
    if let Some(user_seed) = explicit_seed_value {
        parsed_cfg.seed = Some(user_seed);
    }
    parsed_cfg.engine.mode = EngineMode::Batch;

    // Resolve the effective seed: use the explicit value or derive one deterministically.
    let creation_stamp = EventWriter::timestamp();
    let resolved_seed = parsed_cfg
        .seed
        .unwrap_or_else(|| stable_hash_bytes(format!("{creation_stamp}\n{raw_toml}").as_bytes()));
    parsed_cfg.seed = Some(resolved_seed);

    // Select halting Turing machine strategies where applicable.
    parsed_cfg = try_select_halting_turing_machine_strategies(parsed_cfg)
        .map_err(|halting_error| anyhow::anyhow!(halting_error))?;
    // Validate accelerator compatibility before execution.
    accelerator_run_preflight(
        &parsed_cfg,
        parsed_cfg.save_data && parsed_cfg.event_log.enabled,
        parsed_cfg.save_data && parsed_cfg.history.enabled,
        false,
    )
    .map_err(|validation_error| anyhow::anyhow!(validation_error))?;

    let content_hash_id = nit_games::run_id_from_seed_config(resolved_seed, &raw_toml);

    Ok(PreparedBatchConfig {
        resolved_path: canonical_path,
        source_text: raw_toml,
        normalized: parsed_cfg,
        batch_timestamp: creation_stamp,
        effective_seed: resolved_seed,
        deterministic_run_id: content_hash_id,
    })
}

fn log_run_preamble(toml_location: &Path, storage_layout: &Option<RunLayout>) {
    eprintln!(
        "[{}] Games config: {}",
        RunPhase::ConfigPrep,
        toml_location.display()
    );
    match storage_layout.as_ref() {
        Some(available_layout) => {
            eprintln!(
                "[{}] Games summary: {}",
                RunPhase::SummaryEmit,
                available_layout.summary_path.display(),
            );
        }
        None => eprintln!(
            "[{}] Games summary: disabled (`save_data = false`)",
            RunPhase::SummaryEmit,
        ),
    }
}

fn build_run_paths(
    storage_layout: &Option<RunLayout>,
    tournament_output: &super::TournamentRun,
) -> RunPaths {
    let format_path = |extractor: fn(&RunLayout) -> &std::path::Path| -> Option<String> {
        storage_layout
            .as_ref()
            .map(|layout| extractor(layout).display().to_string())
    };

    RunPaths {
        summary: format_path(|l| &l.summary_path),
        definitions: format_path(|l| &l.definitions_path),
        results: format_path(|l| &l.results_path),
        config: format_path(|l| &l.config_path),
        analysis_dir: format_path(|l| &l.analysis_dir),
        events: tournament_output.event_log_path.clone(),
        history: tournament_output.history_log_path.clone(),
    }
}

fn build_headless_summary(
    batch_cfg: &PreparedBatchConfig,
    engine_snapshot: NormalizedConfig,
    storage_layout: &Option<RunLayout>,
    compiled_strategies: &[StrategyDefinition],
    tournament_output: super::TournamentRun,
) -> RunSummary {
    RunSummary {
        schema_version: RUN_SUMMARY_SCHEMA_VERSION,
        timestamp: batch_cfg.batch_timestamp.clone(),
        run_id: batch_cfg.deterministic_run_id.clone(),
        seed: batch_cfg.effective_seed,
        config_text: batch_cfg.source_text.clone(),
        config: engine_snapshot,
        paths: build_run_paths(storage_layout, &tournament_output),
        strategies: compiled_strategies.to_vec(),
        results: tournament_output.results,
        event_log: tournament_output.event_log_path,
        history_log: tournament_output.history_log_path,
        runtime: tournament_output.runtime,
        run_dir: storage_layout
            .as_ref()
            .map(|fs_layout| fs_layout.run_dir.display().to_string()),
    }
}

/// Write the completed summary to disk and optionally emit it to stdout.
fn persist_and_emit_summary(
    final_report: &RunSummary,
    storage_layout: &Option<RunLayout>,
    serialization_format: OutputFormat,
    silent_mode: bool,
    diagnostic_output: bool,
) -> anyhow::Result<()> {
    if let Some(ref writable_layout) = storage_layout {
        let report_destination = &writable_layout.summary_path;
        nit_games::output::write_summary(report_destination, final_report)
            .with_context(|| format!("failed to write {}", report_destination.display()))?;
    }

    if diagnostic_output {
        emit_diagnostic_paths(final_report);
    }

    if !silent_mode {
        let serialized_output = match serialization_format {
            OutputFormat::Json => serde_json::to_string(final_report)?,
            OutputFormat::Pretty => serde_json::to_string_pretty(final_report)?,
        };
        println!("{serialized_output}");
    }

    Ok(())
}

fn emit_diagnostic_paths(report: &RunSummary) {
    if let Some(ref event_path) = report.paths.events {
        eprintln!("Events: {event_path}");
    }
    if let Some(ref history_path) = report.paths.history {
        eprintln!("History: {history_path}");
    }
}
