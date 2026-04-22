//! Headless tournament runner: load a games config, execute the tournament,
//! write artifacts, and emit a summary.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use nit_games::events::EventWriter;
use nit_games::output::{
    RunLayout, RunPaths, RunSummary, StrategyDefinition, RUN_SUMMARY_SCHEMA_VERSION,
};
use nit_games::tournament::TournamentKernel;
use nit_games::{config::EngineMode, NormalizedConfig};
use nit_utils::hashing::stable_hash_bytes;

use crate::cli::{OutputFormat, RunArgs};

const SAVE_DATA_REQUIRED_MSG: &str = "`save_data = false` is not supported for `games run`.";

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

pub(super) fn run_games_headless(args: RunArgs) -> anyhow::Result<()> {
    let RunArgs {
        config,
        strategies,
        out,
        seed,
        output,
    } = args;
    let format = output.format;
    let quiet = output.quiet;
    let verbose = output.verbose;

    let prep = prepare_batch_config(config, strategies, seed)?;
    let artifact_root = super::resolve_output_dir(&prep.resolved_path, out)?;

    let layout = prep.normalized.save_data.then(|| {
        RunLayout::for_base(
            &artifact_root,
            &prep.batch_timestamp,
            prep.effective_seed,
            &prep.deterministic_run_id,
        )
    });

    if let Some(disk) = layout.as_ref() {
        fs::create_dir_all(&disk.run_dir)
            .with_context(|| format!("failed to create {}", disk.run_dir.display()))?;
    }

    if verbose {
        log_run_preamble(&prep.resolved_path, layout.as_ref());
    }

    let engine = TournamentKernel::new(prep.normalized.clone());
    let frozen_config = engine.config().clone();
    let emit_events = prep.normalized.save_data && prep.normalized.event_log.enabled;
    let emit_history = prep.normalized.save_data && prep.normalized.history.enabled;

    let outcome = super::execute_tournament(
        &engine,
        emit_events
            .then(|| layout.as_ref().map(|disk| disk.events_path.clone()))
            .flatten(),
        emit_history
            .then(|| layout.as_ref().map(|disk| disk.history_path.clone()))
            .flatten(),
    )?;

    if let Some(disk) = layout.as_ref() {
        super::write_run_artifacts(
            &disk.config_path,
            &prep.source_text,
            &disk.definitions_path,
            engine.definitions(),
            &disk.results_path,
            &outcome.results,
        );
    }

    let report = build_headless_summary(
        prep,
        frozen_config,
        layout.as_ref(),
        engine.definitions(),
        outcome,
    );

    persist_and_emit_summary(&report, layout.as_ref(), format, quiet, verbose)
}

fn prepare_batch_config(
    toml_source: Option<PathBuf>,
    sidecar_source: Option<PathBuf>,
    explicit_seed: Option<u64>,
) -> anyhow::Result<PreparedBatchConfig> {
    let (canonical_path, raw_toml, mut parsed) =
        super::load_games_config(toml_source, sidecar_source)?;

    if !parsed.save_data {
        anyhow::bail!(SAVE_DATA_REQUIRED_MSG);
    }

    if let Some(user_seed) = explicit_seed {
        parsed.seed = Some(user_seed);
    }
    parsed.engine.mode = EngineMode::Batch;

    let stamp = EventWriter::timestamp();
    let resolved_seed = parsed
        .seed
        .unwrap_or_else(|| stable_hash_bytes(format!("{stamp}\n{raw_toml}").as_bytes()));
    parsed.seed = Some(resolved_seed);

    parsed = super::finalize_config(parsed)?;

    let run_id = nit_games::run_id_from_seed_config(resolved_seed, &raw_toml);

    Ok(PreparedBatchConfig {
        resolved_path: canonical_path,
        source_text: raw_toml,
        normalized: parsed,
        batch_timestamp: stamp,
        effective_seed: resolved_seed,
        deterministic_run_id: run_id,
    })
}

fn log_run_preamble(toml_location: &Path, disk: Option<&RunLayout>) {
    eprintln!("[config-prep] Games config: {}", toml_location.display());
    match disk {
        Some(layout) => eprintln!(
            "[summary-emit] Games summary: {}",
            layout.summary_path.display(),
        ),
        None => eprintln!("[summary-emit] Games summary: disabled (`save_data = false`)"),
    }
}

fn build_run_paths(disk: Option<&RunLayout>, outcome: &super::TournamentRun) -> RunPaths {
    let mut paths = RunPaths {
        summary: None,
        definitions: None,
        results: None,
        config: None,
        analysis_dir: None,
        events: outcome.event_log_path.clone(),
        history: outcome.history_log_path.clone(),
    };
    if let Some(layout) = disk {
        paths.summary = Some(layout.summary_path.display().to_string());
        paths.definitions = Some(layout.definitions_path.display().to_string());
        paths.results = Some(layout.results_path.display().to_string());
        paths.config = Some(layout.config_path.display().to_string());
        paths.analysis_dir = Some(layout.analysis_dir.display().to_string());
    }
    paths
}

fn build_headless_summary(
    prep: PreparedBatchConfig,
    engine_snapshot: NormalizedConfig,
    disk: Option<&RunLayout>,
    definitions: &[StrategyDefinition],
    outcome: super::TournamentRun,
) -> RunSummary {
    let PreparedBatchConfig {
        source_text,
        batch_timestamp,
        effective_seed,
        deterministic_run_id,
        ..
    } = prep;
    let paths = build_run_paths(disk, &outcome);
    let run_dir = disk.map(|layout| layout.run_dir.display().to_string());
    RunSummary {
        schema_version: RUN_SUMMARY_SCHEMA_VERSION,
        timestamp: batch_timestamp,
        run_id: deterministic_run_id,
        seed: effective_seed,
        config_text: source_text,
        config: engine_snapshot,
        paths,
        strategies: definitions.to_vec(),
        results: outcome.results,
        event_log: outcome.event_log_path,
        history_log: outcome.history_log_path,
        runtime: outcome.runtime,
        run_dir,
    }
}

fn persist_and_emit_summary(
    report: &RunSummary,
    disk: Option<&RunLayout>,
    output_format: OutputFormat,
    suppress_stdout: bool,
    verbose: bool,
) -> anyhow::Result<()> {
    if let Some(layout) = disk {
        let target = &layout.summary_path;
        nit_games::output::write_summary(target, report)
            .with_context(|| format!("failed to write {}", target.display()))?;
    }

    if verbose {
        if let Some(events) = report.paths.events.as_deref() {
            eprintln!("Events: {events}");
        }
        if let Some(history) = report.paths.history.as_deref() {
            eprintln!("History: {history}");
        }
    }

    if !suppress_stdout {
        let serialized = match output_format {
            OutputFormat::Json => serde_json::to_string(report)?,
            OutputFormat::Pretty => serde_json::to_string_pretty(report)?,
        };
        println!("{serialized}");
    }

    Ok(())
}
