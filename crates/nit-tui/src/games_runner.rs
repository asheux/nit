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
use nit_games::{HistoryWriter, NormalizedConfig};

#[derive(Clone)]
pub struct RunRequest {
    pub config: NormalizedConfig,
    pub config_text: String,
    pub timestamp: String,
    pub run_id: String,
    pub run_dir: PathBuf,
    pub summary_path: PathBuf,
    pub definitions_path: PathBuf,
    pub results_path: PathBuf,
    pub config_path: PathBuf,
    pub analysis_dir: PathBuf,
    pub event_path: Option<PathBuf>,
    pub history_path: Option<PathBuf>,
    pub progress_interval: Duration,
    pub steps_per_tick: u32,
}

pub enum RunnerCommand {
    StartRun(RunRequest),
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
    Finished(RunSummary),
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
    run_dir: PathBuf,
    summary_path: PathBuf,
    definitions_path: PathBuf,
    results_path: PathBuf,
    config_path: PathBuf,
    analysis_dir: PathBuf,
    steps_per_tick: u32,
    progress_interval: Duration,
    last_progress: Instant,
    last_progress_match: Option<usize>,
    last_completed: usize,
}

fn runner_loop(cmd_rx: Receiver<RunnerCommand>, event_tx: Sender<RunnerEvent>) {
    let mut state: Option<RunState> = None;
    let mut paused = false;

    loop {
        if state.is_none() {
            match cmd_rx.recv() {
                Ok(RunnerCommand::StartRun(request)) => match start_run(request, &event_tx) {
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
            dephase_steps_per_tick(run_state.steps_per_tick, rounds_per_match)
        };
        run_state.runner.step_rounds(steps);
        for preview in run_state.runner.drain_match_history_previews() {
            let _ = event_tx.send(RunnerEvent::MatchHistoryPreview(preview));
        }

        let completed = run_state.runner.completed_matches();
        if completed > run_state.last_completed {
            let results = run_state.runner.results();
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
            let _ = event_tx.send(RunnerEvent::PartialLeaderboard(run_state.runner.results()));
            match finalize_run(state.take().expect("run state")) {
                Ok(summary) => {
                    let _ = event_tx.send(RunnerEvent::Finished(summary));
                }
                Err(err) => {
                    let _ = event_tx.send(RunnerEvent::Error(err));
                }
            }
        }
    }
}

fn start_run(request: RunRequest, event_tx: &Sender<RunnerEvent>) -> Result<RunState, String> {
    let mut runner = TournamentRunner::new(request.config);
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
    })
}

fn finalize_run(state: RunState) -> Result<RunSummary, String> {
    if let Err(err) = std::fs::write(&state.config_path, &state.config_text) {
        tracing::warn!("Failed to write games config snapshot: {err}");
    }

    let mut summary = state.runner.finish(
        state.timestamp.clone(),
        state.run_id.clone(),
        state.config_text.clone(),
    );
    summary.schema_version = RUN_SUMMARY_SCHEMA_VERSION;
    summary.paths.summary = Some(state.summary_path.display().to_string());
    summary.paths.definitions = Some(state.definitions_path.display().to_string());
    summary.paths.results = Some(state.results_path.display().to_string());
    summary.paths.config = Some(state.config_path.display().to_string());
    summary.paths.analysis_dir = Some(state.analysis_dir.display().to_string());
    summary.run_dir = Some(state.run_dir.display().to_string());
    summary.event_log = summary.paths.events.clone();
    summary.history_log = summary.paths.history.clone();

    if let Err(err) = nit_utils::fs::write_atomic(&state.definitions_path, |writer| {
        serde_json::to_writer_pretty(writer, &summary.strategies)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }) {
        tracing::warn!("Failed to write games definitions: {err}");
    }
    if let Err(err) = nit_utils::fs::write_atomic(&state.results_path, |writer| {
        serde_json::to_writer_pretty(writer, &summary.results)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }) {
        tracing::warn!("Failed to write games results: {err}");
    }
    write_summary(&state.summary_path, &summary).map_err(|err| err.to_string())?;
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
    if steps % rounds_per_match == 0 {
        steps.saturating_add(1)
    } else {
        steps
    }
}

#[cfg(test)]
mod tests {
    use super::dephase_steps_per_tick;

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
}
