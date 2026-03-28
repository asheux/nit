use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nit_games::events::EventWriter;
use nit_games::output::{
    write_summary, RunSummary, StrategyDefinition, TournamentResults, RUN_SUMMARY_SCHEMA_VERSION,
};
use nit_games::tournament::{
    MatchHistoryPreview, MatchSnapshot, TournamentProgress, TournamentRunner,
};
use nit_games::{
    accelerator_run_preflight, try_select_halting_turing_machine_strategies_with_diagnostics,
    EngineMode, HistoryWriter, NormalizedConfig, TmHaltingFilterDiagnostics,
};
use tracing::info;

const RUNNER_CHUNK_TARGET: Duration = Duration::from_millis(120);
const RUNNER_CHUNK_INITIAL: u32 = 4_096;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum StepUnit {
    Rounds,
    Matches,
}

#[derive(Clone)]
pub struct RunRequest {
    pub config: NormalizedConfig,
    pub config_text: String,
    pub timestamp: String,
    pub run_id: String,
    pub run_dir: Option<PathBuf>,
    pub summary_path: Option<PathBuf>,
    pub definitions_path: Option<PathBuf>,
    pub results_path: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub analysis_dir: Option<PathBuf>,
    pub event_path: Option<PathBuf>,
    pub history_path: Option<PathBuf>,
    pub progress_interval: Duration,
    pub steps_per_tick: u32,
}

pub enum RunnerCommand {
    StartRun(Box<RunRequest>),
    Pause,
    Resume,
    StepOnce,
    Cancel,
    UpdateSpeed(u32),
    Shutdown,
}

pub enum RunnerEvent {
    StartupStage(String),
    Definitions(Vec<StrategyDefinition>),
    Progress(TournamentProgress),
    MatchPreview(MatchSnapshot),
    MatchHistoryPreview(MatchHistoryPreview),
    PartialLeaderboard(TournamentResults),
    Finished(Box<RunSummary>),
    Cancelled,
    Error(String),
}

pub struct GamesRunner {
    cmd_tx: Sender<RunnerCommand>,
    pub events: Receiver<RunnerEvent>,
    handle: Option<JoinHandle<()>>,
}

impl GamesRunner {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("nit-games-runner".into())
            .spawn(move || runner_loop(cmd_rx, event_tx))
            .expect("spawn games runner");
        Self {
            cmd_tx,
            events: event_rx,
            handle: Some(handle),
        }
    }

    pub fn send(&self, command: RunnerCommand) {
        let _ = self.cmd_tx.send(command);
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(RunnerCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct RunState {
    runner: TournamentRunner,
    config_text: String,
    timestamp: String,
    run_id: String,
    run_dir: Option<PathBuf>,
    summary_path: Option<PathBuf>,
    definitions_path: Option<PathBuf>,
    results_path: Option<PathBuf>,
    config_path: Option<PathBuf>,
    analysis_dir: Option<PathBuf>,
    steps_per_tick: u32,
    progress_interval: Duration,
    last_progress: Instant,
    last_progress_match: Option<usize>,
    last_completed: usize,
    chunk_hint_steps: Option<u32>,
    step_unit: StepUnit,
}

fn runner_loop(cmd_rx: Receiver<RunnerCommand>, event_tx: Sender<RunnerEvent>) {
    let mut state: Option<RunState> = None;
    let mut paused = false;

    loop {
        if state.is_none() {
            match cmd_rx.recv() {
                Ok(RunnerCommand::StartRun(request)) => match start_run(*request, &event_tx) {
                    Ok(run_state) => {
                        state = Some(run_state);
                        paused = false;
                    }
                    Err(err) => {
                        let _ = event_tx.send(RunnerEvent::Error(err));
                    }
                },
                Ok(RunnerCommand::Shutdown) | Err(_) => break,
                _ => {}
            }
            continue;
        }

        let mut step_once = false;
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                RunnerCommand::Pause => paused = true,
                RunnerCommand::Resume => paused = false,
                RunnerCommand::StepOnce => step_once = true,
                RunnerCommand::Cancel => {
                    if let Some(run_state) = state.take() {
                        finalize_cancel(run_state);
                    }
                    let _ = event_tx.send(RunnerEvent::Cancelled);
                    paused = false;
                }
                RunnerCommand::UpdateSpeed(steps) => {
                    if let Some(run_state) = state.as_mut() {
                        run_state.steps_per_tick = steps.max(1);
                    }
                }
                RunnerCommand::Shutdown => {
                    if let Some(run_state) = state.take() {
                        finalize_cancel(run_state);
                    }
                    return;
                }
                RunnerCommand::StartRun(_) => {}
            }
        }

        if state.is_none() {
            continue;
        }

        if paused && !step_once {
            thread::sleep(Duration::from_millis(20));
            continue;
        }

        let Some(run_state) = state.as_mut() else {
            continue;
        };

        let rounds_per_match = run_state.runner.config().rounds.max(1);
        let mode = run_state.runner.config().engine.mode;
        let whole_match_chunks =
            matches!(mode, EngineMode::Batch) || matches!(run_state.step_unit, StepUnit::Matches);
        let steps = if step_once {
            1
        } else {
            steps_for_tick(
                requested_steps_per_tick(
                    run_state.steps_per_tick,
                    rounds_per_match,
                    run_state.step_unit,
                ),
                rounds_per_match,
                mode,
                whole_match_chunks,
            )
        };
        let chunk_steps = if step_once {
            1
        } else {
            adaptive_chunk_steps(
                steps,
                run_state.chunk_hint_steps,
                rounds_per_match,
                mode,
                whole_match_chunks,
            )
        };
        let chunk_started_at = Instant::now();
        run_state.runner.step_rounds(chunk_steps);
        if !step_once {
            run_state.chunk_hint_steps = Some(next_adaptive_chunk_steps(
                chunk_steps,
                chunk_started_at.elapsed(),
                rounds_per_match,
                mode,
                whole_match_chunks,
            ));
        }
        for preview in run_state.runner.drain_match_history_previews() {
            let _ = event_tx.send(RunnerEvent::MatchHistoryPreview(preview));
        }

        let completed = run_state.runner.completed_matches();
        if completed > run_state.last_completed {
            let results = run_state.runner.leaderboard();
            let _ = event_tx.send(RunnerEvent::PartialLeaderboard(results));
            run_state.last_completed = completed;
        }

        let progress = run_state.runner.progress();
        let progress_match = progress.as_ref().map(|p| p.match_index);
        let match_changed = progress_match != run_state.last_progress_match;
        if run_state.progress_interval.is_zero()
            || run_state.last_progress.elapsed() >= run_state.progress_interval
            || match_changed
        {
            if let Some(progress) = progress {
                let _ = event_tx.send(RunnerEvent::Progress(progress));
            }
            if let Some(snapshot) = run_state.runner.match_snapshot() {
                let _ = event_tx.send(RunnerEvent::MatchPreview(snapshot));
            }
            run_state.last_progress = Instant::now();
            run_state.last_progress_match = progress_match;
        }

        if run_state.runner.is_done() {
            if let Some(progress) = run_state.runner.progress() {
                let _ = event_tx.send(RunnerEvent::Progress(progress));
            }
            if let Some(snapshot) = run_state.runner.match_snapshot() {
                let _ = event_tx.send(RunnerEvent::MatchPreview(snapshot));
            }
            let _ = event_tx.send(RunnerEvent::PartialLeaderboard(
                run_state.runner.leaderboard(),
            ));
            match finalize_run(state.take().expect("run state")) {
                Ok(summary) => {
                    let _ = event_tx.send(RunnerEvent::Finished(Box::new(summary)));
                }
                Err(err) => {
                    let _ = event_tx.send(RunnerEvent::Error(err));
                }
            }
        }
    }
}

fn startup_schedule_match_count(config: &NormalizedConfig) -> usize {
    let strategy_count = config.strategies.len();
    let per_rep = if config.self_play {
        strategy_count.checked_mul(strategy_count)
    } else {
        strategy_count.checked_mul(strategy_count.saturating_sub(1))
    };
    per_rep
        .and_then(|matches| matches.checked_mul(config.repetitions as usize))
        .unwrap_or(0)
}

fn tm_filter_stage_message(diag: &TmHaltingFilterDiagnostics) -> String {
    let mut message = format!(
        "TM filter [{}] (requested {:?}): kept {}/{}; scanned {} of {} matchups; probe {:?}; filter {:?}",
        diag.backend.label(),
        diag.requested_accelerator,
        diag.strategy_count_after,
        diag.strategy_count_before,
        diag.scanned_matchups,
        diag.schedule_matches,
        diag.backend_probe_elapsed,
        diag.halting_filter_elapsed
    );
    if diag.tm_evaluations > 0 || diag.tm_cache_hits > 0 {
        message.push_str(&format!(
            "; TM evals {} (hits {}, misses {}, steps {})",
            diag.tm_evaluations, diag.tm_cache_hits, diag.tm_cache_misses, diag.tm_steps
        ));
    }
    if let Some(reason) = diag.metal_decline_reason.as_ref() {
        message.push_str(&format!("; metal decline: {reason}"));
    }
    if let Some(err) = diag.metal_error.as_ref() {
        message.push_str(&format!("; metal error: {err}"));
    }
    if diag.metal_batches_submitted > 0 {
        message.push_str(&format!("; metal batches {}", diag.metal_batches_submitted));
    }
    if let Some(source) = diag.metal_policy_source.as_ref() {
        message.push_str(&format!("; metal policy {source}"));
    }
    if let (Some(matches_per_batch), Some(inflight)) =
        (diag.metal_matches_per_batch, diag.metal_inflight_batches)
    {
        message.push_str(&format!(
            "; batch {matches_per_batch}x in-flight {inflight}"
        ));
    }
    message
}

pub(crate) fn uses_match_step_units(
    config: &NormalizedConfig,
    event_logging: bool,
    history_logging: bool,
    match_history_previews: bool,
) -> bool {
    matches!(config.engine.mode, EngineMode::Interactive)
        && config.engine.fast_eval
        && config.noise == 0.0
        && !event_logging
        && !history_logging
        && !match_history_previews
}

fn start_run(request: RunRequest, event_tx: &Sender<RunnerEvent>) -> Result<RunState, String> {
    let startup_started = Instant::now();
    let requested_strategies = request.config.strategies.len();
    let requested_matches = startup_schedule_match_count(&request.config);
    let _ = event_tx.send(RunnerEvent::StartupStage(format!(
        "Selecting halting TMs ({requested_strategies} strategies, {requested_matches} scheduled matches)..."
    )));

    let halting_started = Instant::now();
    let (config, diagnostics) =
        try_select_halting_turing_machine_strategies_with_diagnostics(request.config)?;
    let halting_elapsed = halting_started.elapsed();
    let stage_summary = tm_filter_stage_message(&diagnostics);
    let _ = event_tx.send(RunnerEvent::StartupStage(stage_summary.clone()));
    info!(
        "Games runner startup TM filter complete: {stage_summary}; total {:?}",
        diagnostics.total_elapsed
    );

    // Metal batch dispatch requires fast_forward_allowed() which is blocked by
    // match history previews, event writers, and history writers.  Metal only
    // produces final scores — it cannot emit per-round events or history.
    // When Metal acceleration is available, disable these to let Metal work.
    let metal_path = config.engine.accelerator.allows_metal();
    let use_previews = !matches!(config.engine.mode, EngineMode::Batch) && !metal_path;
    let use_event_log = request.event_path.is_some() && !metal_path;
    let use_history_log = request.history_path.is_some() && !metal_path;
    let step_unit = if uses_match_step_units(&config, use_event_log, use_history_log, use_previews)
    {
        StepUnit::Matches
    } else {
        StepUnit::Rounds
    };
    info!(
        "Games runner Metal path: metal_path={metal_path}, accelerator={:?}, mode={:?}, \
         fast_eval={}, noise={}, previews={use_previews}, event_log={use_event_log}, \
         history_log={use_history_log}, step_unit={:?}",
        config.engine.accelerator,
        config.engine.mode,
        config.engine.fast_eval,
        config.noise,
        step_unit,
    );

    let _ = event_tx.send(RunnerEvent::StartupStage(
        "Preparing backend and preview mode...".into(),
    ));
    let preflight_started = Instant::now();
    accelerator_run_preflight(&config, use_event_log, use_history_log, use_previews)?;
    let preflight_elapsed = preflight_started.elapsed();

    let _ = event_tx.send(RunnerEvent::StartupStage(
        "Building tournament runner...".into(),
    ));
    let runner_started = Instant::now();
    let mut runner = TournamentRunner::new(config);
    let runner_elapsed = runner_started.elapsed();
    if !use_previews {
        runner = runner.with_match_history_previews(false);
    }
    if use_event_log {
        if let Some(path) = request.event_path {
            let include_rounds = runner.config().event_log.include_rounds;
            match EventWriter::new(path, include_rounds) {
                Ok(writer) => runner = runner.with_event_writer(writer),
                Err(err) => return Err(format!("Failed to create event log: {err}")),
            }
        }
    }
    if use_history_log {
        if let Some(path) = request.history_path {
            match HistoryWriter::new(path) {
                Ok(writer) => runner = runner.with_history_writer(writer),
                Err(err) => return Err(format!("Failed to create history log: {err}")),
            }
        }
    }

    let definitions = runner.definitions().to_vec();
    let _ = event_tx.send(RunnerEvent::Definitions(definitions));
    info!(
        "Games runner startup complete in {:?} (halting {:?}, preflight {:?}, runner {:?})",
        startup_started.elapsed(),
        halting_elapsed,
        preflight_elapsed,
        runner_elapsed
    );

    // When using match-unit stepping (Metal fast path), ensure a reasonable
    // minimum so the adaptive chunk system has room to grow.  With the default
    // steps_per_tick of 1 and high rounds_per_match, requested_steps saturates
    // at 1 match per tick and the adaptive system can never grow beyond that,
    // causing the tournament to crawl.
    let effective_steps_per_tick = if matches!(step_unit, StepUnit::Matches) {
        request.steps_per_tick.max(64)
    } else {
        request.steps_per_tick.max(1)
    };

    Ok(RunState {
        runner,
        config_text: request.config_text,
        timestamp: request.timestamp,
        run_id: request.run_id,
        run_dir: request.run_dir,
        summary_path: request.summary_path,
        definitions_path: request.definitions_path,
        results_path: request.results_path,
        config_path: request.config_path,
        analysis_dir: request.analysis_dir,
        steps_per_tick: effective_steps_per_tick,
        progress_interval: request.progress_interval,
        last_progress: Instant::now(),
        last_progress_match: None,
        last_completed: 0,
        chunk_hint_steps: None,
        step_unit,
    })
}

fn finalize_run(state: RunState) -> Result<RunSummary, String> {
    if let Some(config_path) = state.config_path.as_ref() {
        if let Err(err) = std::fs::write(config_path, &state.config_text) {
            tracing::warn!("Failed to write games config snapshot: {err}");
        }
    }

    let mut summary = state.runner.finish(
        state.timestamp.clone(),
        state.run_id.clone(),
        state.config_text.clone(),
    );
    summary.schema_version = RUN_SUMMARY_SCHEMA_VERSION;
    summary.paths.summary = state
        .summary_path
        .as_ref()
        .map(|path| path.display().to_string());
    summary.paths.definitions = state
        .definitions_path
        .as_ref()
        .map(|path| path.display().to_string());
    summary.paths.results = state
        .results_path
        .as_ref()
        .map(|path| path.display().to_string());
    summary.paths.config = state
        .config_path
        .as_ref()
        .map(|path| path.display().to_string());
    summary.paths.analysis_dir = state
        .analysis_dir
        .as_ref()
        .map(|path| path.display().to_string());
    summary.run_dir = state
        .run_dir
        .as_ref()
        .map(|path| path.display().to_string());
    summary.event_log = summary.paths.events.clone();
    summary.history_log = summary.paths.history.clone();

    if let Some(definitions_path) = state.definitions_path.as_ref() {
        if let Err(err) = nit_utils::fs::write_atomic(definitions_path, |writer| {
            serde_json::to_writer_pretty(writer, &summary.strategies).map_err(std::io::Error::other)
        }) {
            tracing::warn!("Failed to write games definitions: {err}");
        }
    }
    if let Some(results_path) = state.results_path.as_ref() {
        if let Err(err) = nit_utils::fs::write_atomic(results_path, |writer| {
            serde_json::to_writer_pretty(writer, &summary.results).map_err(std::io::Error::other)
        }) {
            tracing::warn!("Failed to write games results: {err}");
        }
    }
    if let Some(summary_path) = state.summary_path.as_ref() {
        write_summary(summary_path, &summary).map_err(|err| err.to_string())?;
    }
    Ok(summary)
}

fn finalize_cancel(state: RunState) {
    let _ = state
        .runner
        .finish(state.timestamp, state.run_id, state.config_text);
}

fn dephase_steps_per_tick(steps_per_tick: u32, rounds_per_match: u32) -> u32 {
    let steps = steps_per_tick.max(1);
    if rounds_per_match <= 1 || steps <= 1 {
        return steps;
    }
    if steps.is_multiple_of(rounds_per_match) {
        steps.saturating_add(1)
    } else {
        steps
    }
}

fn requested_steps_per_tick(
    steps_per_tick: u32,
    rounds_per_match: u32,
    step_unit: StepUnit,
) -> u32 {
    match step_unit {
        StepUnit::Rounds => steps_per_tick.max(1),
        StepUnit::Matches => steps_per_tick
            .max(1)
            .saturating_mul(rounds_per_match.max(1)),
    }
}

fn steps_for_tick(
    requested_steps: u32,
    rounds_per_match: u32,
    mode: EngineMode,
    whole_match_chunks: bool,
) -> u32 {
    if whole_match_chunks || matches!(mode, EngineMode::Batch) {
        requested_steps.max(1)
    } else {
        dephase_steps_per_tick(requested_steps, rounds_per_match)
    }
}

fn adaptive_chunk_steps(
    requested_steps: u32,
    chunk_hint_steps: Option<u32>,
    rounds_per_match: u32,
    mode: EngineMode,
    whole_match_chunks: bool,
) -> u32 {
    let requested_steps = requested_steps.max(1);
    let base = chunk_hint_steps.unwrap_or_else(|| {
        requested_steps.min(initial_chunk_steps(
            rounds_per_match,
            mode,
            whole_match_chunks,
        ))
    });
    normalize_chunk_steps(
        base,
        requested_steps,
        rounds_per_match,
        mode,
        whole_match_chunks,
    )
}

fn next_adaptive_chunk_steps(
    chunk_steps: u32,
    chunk_elapsed: Duration,
    rounds_per_match: u32,
    mode: EngineMode,
    whole_match_chunks: bool,
) -> u32 {
    let min_steps = minimum_chunk_steps(rounds_per_match, mode, whole_match_chunks);
    let elapsed_nanos = chunk_elapsed.as_nanos().max(1);
    let target_nanos = RUNNER_CHUNK_TARGET.as_nanos();
    let scaled = ((chunk_steps as u128).saturating_mul(target_nanos) / elapsed_nanos)
        .min(u32::MAX as u128) as u32;
    let shrink_floor = (chunk_steps / 2).max(min_steps);
    let growth_ceiling = chunk_steps.saturating_mul(4).max(min_steps);
    normalize_chunk_steps(
        scaled.clamp(shrink_floor, growth_ceiling),
        u32::MAX,
        rounds_per_match,
        mode,
        whole_match_chunks,
    )
}

fn initial_chunk_steps(rounds_per_match: u32, mode: EngineMode, whole_match_chunks: bool) -> u32 {
    let rounds_per_match = rounds_per_match.max(1);
    if whole_match_chunks || matches!(mode, EngineMode::Batch) {
        // Start with enough steps for a meaningful Metal batch.
        // With large rounds (e.g. 500K), RUNNER_CHUNK_INITIAL (4096) is less
        // than one match, so the adaptive chunk gets stuck at minimum.  Start
        // with at least 256 matches worth of rounds to let Metal batching work
        // from the first tick.
        rounds_per_match
            .saturating_mul(256)
            .max(RUNNER_CHUNK_INITIAL)
    } else {
        rounds_per_match
            .saturating_mul(16)
            .clamp(256, RUNNER_CHUNK_INITIAL)
    }
}

fn minimum_chunk_steps(rounds_per_match: u32, mode: EngineMode, whole_match_chunks: bool) -> u32 {
    if whole_match_chunks || matches!(mode, EngineMode::Batch) {
        rounds_per_match.max(1)
    } else {
        1
    }
}

fn normalize_chunk_steps(
    candidate_steps: u32,
    requested_steps: u32,
    rounds_per_match: u32,
    mode: EngineMode,
    whole_match_chunks: bool,
) -> u32 {
    let requested_steps = requested_steps.max(1);
    let min_steps =
        minimum_chunk_steps(rounds_per_match, mode, whole_match_chunks).min(requested_steps);
    let chunk_steps = candidate_steps.clamp(min_steps, requested_steps);
    // When using whole-match chunks (Metal batch or match-unit stepping),
    // round down to exact match boundaries so fast_forward can consume all
    // steps without leaving a fractional-match remainder that would force
    // a slow round-by-round fallback.  Without this, high round counts
    // combined with u32 saturation in the adaptive system leave a remainder
    // (e.g. u32::MAX % 500_000 = 467_295 rounds) that gets processed
    // round-by-round on the CPU every tick.
    if whole_match_chunks && rounds_per_match > 1 {
        let aligned = (chunk_steps / rounds_per_match).saturating_mul(rounds_per_match);
        return aligned.max(min_steps);
    }
    if !matches!(mode, EngineMode::Interactive)
        || rounds_per_match <= 1
        || chunk_steps <= 1
        || !chunk_steps.is_multiple_of(rounds_per_match)
    {
        return chunk_steps;
    }
    if chunk_steps < requested_steps {
        chunk_steps.saturating_add(1)
    } else {
        chunk_steps.saturating_sub(1).max(1)
    }
}

#[cfg(test)]
#[path = "tests/games_runner.rs"]
mod tests;
