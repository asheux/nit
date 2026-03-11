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
use nit_games::{accelerator_run_preflight, EngineMode, HistoryWriter, NormalizedConfig};

const RUNNER_CHUNK_TARGET: Duration = Duration::from_millis(120);
const RUNNER_CHUNK_INITIAL: u32 = 4_096;

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
        let steps = if step_once {
            1
        } else {
            steps_for_tick(
                run_state.steps_per_tick,
                rounds_per_match,
                run_state.runner.config().engine.mode,
            )
        };
        let chunk_steps = if step_once {
            1
        } else {
            adaptive_chunk_steps(
                steps,
                run_state.chunk_hint_steps,
                rounds_per_match,
                run_state.runner.config().engine.mode,
            )
        };
        let chunk_started_at = Instant::now();
        run_state.runner.step_rounds(chunk_steps);
        if !step_once {
            run_state.chunk_hint_steps = Some(next_adaptive_chunk_steps(
                chunk_steps,
                chunk_started_at.elapsed(),
                rounds_per_match,
                run_state.runner.config().engine.mode,
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

fn start_run(request: RunRequest, event_tx: &Sender<RunnerEvent>) -> Result<RunState, String> {
    let match_history_previews = !matches!(request.config.engine.mode, EngineMode::Batch);
    accelerator_run_preflight(
        &request.config,
        request.event_path.is_some(),
        request.history_path.is_some(),
        match_history_previews,
    )?;

    let mut runner = TournamentRunner::new(request.config);
    if matches!(runner.config().engine.mode, EngineMode::Batch) {
        runner = runner.with_match_history_previews(false);
    }
    if let Some(path) = request.event_path {
        let include_rounds = runner.config().event_log.include_rounds;
        match EventWriter::new(path, include_rounds) {
            Ok(writer) => runner = runner.with_event_writer(writer),
            Err(err) => return Err(format!("Failed to create event log: {err}")),
        }
    }
    if let Some(path) = request.history_path {
        match HistoryWriter::new(path) {
            Ok(writer) => runner = runner.with_history_writer(writer),
            Err(err) => return Err(format!("Failed to create history log: {err}")),
        }
    }

    let definitions = runner.definitions().to_vec();
    let _ = event_tx.send(RunnerEvent::Definitions(definitions));

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
        steps_per_tick: request.steps_per_tick.max(1),
        progress_interval: request.progress_interval,
        last_progress: Instant::now(),
        last_progress_match: None,
        last_completed: 0,
        chunk_hint_steps: None,
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

fn steps_for_tick(steps_per_tick: u32, rounds_per_match: u32, mode: EngineMode) -> u32 {
    if matches!(mode, EngineMode::Batch) {
        steps_per_tick.max(1)
    } else {
        dephase_steps_per_tick(steps_per_tick, rounds_per_match)
    }
}

fn adaptive_chunk_steps(
    requested_steps: u32,
    chunk_hint_steps: Option<u32>,
    rounds_per_match: u32,
    mode: EngineMode,
) -> u32 {
    let requested_steps = requested_steps.max(1);
    let base = chunk_hint_steps
        .unwrap_or_else(|| requested_steps.min(initial_chunk_steps(rounds_per_match, mode)));
    normalize_chunk_steps(base, requested_steps, rounds_per_match, mode)
}

fn next_adaptive_chunk_steps(
    chunk_steps: u32,
    chunk_elapsed: Duration,
    rounds_per_match: u32,
    mode: EngineMode,
) -> u32 {
    let min_steps = minimum_chunk_steps(rounds_per_match, mode);
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
    )
}

fn initial_chunk_steps(rounds_per_match: u32, mode: EngineMode) -> u32 {
    let rounds_per_match = rounds_per_match.max(1);
    match mode {
        EngineMode::Batch => rounds_per_match
            .saturating_mul(32)
            .max(rounds_per_match)
            .min(RUNNER_CHUNK_INITIAL),
        EngineMode::Interactive => rounds_per_match
            .saturating_mul(16)
            .max(256)
            .min(RUNNER_CHUNK_INITIAL),
    }
}

fn minimum_chunk_steps(rounds_per_match: u32, mode: EngineMode) -> u32 {
    match mode {
        EngineMode::Batch => rounds_per_match.max(1),
        EngineMode::Interactive => 1,
    }
}

fn normalize_chunk_steps(
    candidate_steps: u32,
    requested_steps: u32,
    rounds_per_match: u32,
    mode: EngineMode,
) -> u32 {
    let requested_steps = requested_steps.max(1);
    let min_steps = minimum_chunk_steps(rounds_per_match, mode).min(requested_steps);
    let chunk_steps = candidate_steps.clamp(min_steps, requested_steps);
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
mod tests {
    use super::{
        adaptive_chunk_steps, dephase_steps_per_tick, finalize_run, next_adaptive_chunk_steps,
        start_run, steps_for_tick, RunRequest,
    };
    use nit_games::{EngineMode, GamesConfig};
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn dephase_steps_breaks_round_alignment() {
        assert_eq!(dephase_steps_per_tick(50_000, 20), 50_001);
        assert_eq!(dephase_steps_per_tick(20, 20), 21);
    }

    #[test]
    fn dephase_steps_keeps_non_aligned_values() {
        assert_eq!(dephase_steps_per_tick(50_001, 20), 50_001);
        assert_eq!(dephase_steps_per_tick(1, 20), 1);
        assert_eq!(dephase_steps_per_tick(20, 1), 20);
    }

    #[test]
    fn batch_steps_skip_dephase() {
        assert_eq!(steps_for_tick(50_000, 20, EngineMode::Batch), 50_000);
        assert_eq!(steps_for_tick(20, 20, EngineMode::Interactive), 21);
    }

    #[test]
    fn adaptive_chunk_steps_caps_large_batch_runs_initially() {
        let chunk = adaptive_chunk_steps(250_000, None, 200, EngineMode::Batch);

        assert_eq!(chunk, 4_096);
    }

    #[test]
    fn adaptive_chunk_steps_preserves_batch_match_floor() {
        let chunk = adaptive_chunk_steps(400, Some(25), 200, EngineMode::Batch);

        assert_eq!(chunk, 200);
    }

    #[test]
    fn next_adaptive_chunk_steps_grows_after_fast_chunk() {
        let next =
            next_adaptive_chunk_steps(4_096, Duration::from_millis(30), 200, EngineMode::Batch);

        assert_eq!(next, 16_384);
    }

    #[test]
    fn next_adaptive_chunk_steps_shrinks_after_slow_chunk() {
        let next =
            next_adaptive_chunk_steps(4_096, Duration::from_millis(480), 200, EngineMode::Batch);

        assert_eq!(next, 2_048);
    }

    #[test]
    fn next_adaptive_chunk_steps_can_grow_from_batch_match_floor() {
        let next =
            next_adaptive_chunk_steps(5_000, Duration::from_millis(30), 5_000, EngineMode::Batch);

        assert_eq!(next, 20_000);
    }

    #[test]
    fn start_run_rejects_interactive_metal_requests_that_need_cpu_previews() {
        let config = GamesConfig::from_toml(
            r#"
schema_version = 1
game = "ipd"
rounds = 4
repetitions = 1
self_play = false

[engine]
mode = "interactive"
fast_eval = true
accelerator = "metal"

[event_log]
enabled = false
include_rounds = false

[history]
enabled = false

[[strategy]]
id = "fsm_0"
type = "fsm"
index = 0
num_states = 1
k = 2

[[strategy]]
id = "fsm_1"
type = "fsm"
index = 1
num_states = 1
k = 2
"#,
        )
        .expect("parse config");
        let (event_tx, _event_rx) = mpsc::channel();
        let request = RunRequest {
            config,
            config_text: String::new(),
            timestamp: "2026-03-11T00:00:00Z".into(),
            run_id: "run".into(),
            run_dir: Some(PathBuf::from("/tmp/run")),
            summary_path: Some(PathBuf::from("/tmp/run/run_summary.json")),
            definitions_path: Some(PathBuf::from("/tmp/run/definitions.json")),
            results_path: Some(PathBuf::from("/tmp/run/results.json")),
            config_path: Some(PathBuf::from("/tmp/run/config.toml")),
            analysis_dir: Some(PathBuf::from("/tmp/run/analysis")),
            event_path: None,
            history_path: None,
            progress_interval: Duration::from_millis(10),
            steps_per_tick: 128,
        };

        let err = start_run(request, &event_tx)
            .err()
            .expect("interactive metal should fail fast");
        assert!(err.contains("interactive match history previews"));
    }

    #[test]
    fn finalize_run_keeps_paths_empty_for_ephemeral_runs() {
        let config = GamesConfig::from_toml(
            r#"
schema_version = 1
game = "ipd"
rounds = 4
repetitions = 1
self_play = false
save_data = false

[engine]
mode = "batch"
fast_eval = true
accelerator = "cpu"

[event_log]
enabled = true
include_rounds = false

[history]
enabled = true

[[strategy]]
id = "fsm_0"
type = "fsm"
index = 0
num_states = 1
k = 2
"#,
        )
        .expect("parse config");
        let (event_tx, _event_rx) = mpsc::channel();
        let request = RunRequest {
            config,
            config_text: String::new(),
            timestamp: "2026-03-12T00:00:00Z".into(),
            run_id: "run".into(),
            run_dir: None,
            summary_path: None,
            definitions_path: None,
            results_path: None,
            config_path: None,
            analysis_dir: None,
            event_path: None,
            history_path: None,
            progress_interval: Duration::from_millis(10),
            steps_per_tick: 128,
        };

        let state = start_run(request, &event_tx).expect("ephemeral run should start");
        let summary = finalize_run(state).expect("ephemeral run should finalize");
        assert!(summary.paths.summary.is_none());
        assert!(summary.paths.definitions.is_none());
        assert!(summary.paths.results.is_none());
        assert!(summary.paths.config.is_none());
        assert!(summary.paths.analysis_dir.is_none());
        assert!(summary.run_dir.is_none());
        assert!(summary.event_log.is_none());
        assert!(summary.history_log.is_none());
    }
}
