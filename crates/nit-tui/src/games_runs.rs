use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use nit_core::GamesRunEntry;
use nit_games::game::{Action, PayoffMatrix};
use nit_games::history_log::MatchHistory;
use nit_games::CycleMetadata;
use nit_games::RunSummary;

#[derive(Clone, Debug)]
pub struct ReplayData {
    pub title: String,
    pub lines: Vec<String>,
    pub cycle: Option<CycleMetadata>,
}

pub enum RunsCommand {
    Refresh {
        base_dir: PathBuf,
    },
    LoadSummary {
        summary_path: PathBuf,
    },
    LoadReplay {
        history_path: PathBuf,
        a_id: String,
        b_id: String,
        payoff: PayoffMatrix,
    },
    Shutdown,
}

pub enum RunsEvent {
    RunsLoaded(Vec<GamesRunEntry>),
    SummaryLoaded(Box<RunSummary>),
    ReplayLoaded(ReplayData),
    Error(String),
}

pub struct GamesRunsRunner {
    cmd_tx: Sender<RunsCommand>,
    pub events: Receiver<RunsEvent>,
    handle: Option<JoinHandle<()>>,
}

impl GamesRunsRunner {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("nit-games-runs".into())
            .spawn(move || runner_loop(cmd_rx, event_tx))
            .expect("spawn games runs runner");
        Self {
            cmd_tx,
            events: event_rx,
            handle: Some(handle),
        }
    }

    pub fn send(&self, command: RunsCommand) {
        let _ = self.cmd_tx.send(command);
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(RunsCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn runner_loop(cmd_rx: Receiver<RunsCommand>, event_tx: Sender<RunsEvent>) {
    loop {
        match cmd_rx.recv() {
            Ok(RunsCommand::Refresh { base_dir }) => match scan_runs(&base_dir) {
                Ok(entries) => {
                    let _ = event_tx.send(RunsEvent::RunsLoaded(entries));
                }
                Err(err) => {
                    let _ = event_tx.send(RunsEvent::Error(err));
                }
            },
            Ok(RunsCommand::LoadSummary { summary_path }) => match load_summary(&summary_path) {
                Ok(summary) => {
                    let _ = event_tx.send(RunsEvent::SummaryLoaded(Box::new(summary)));
                }
                Err(err) => {
                    let _ = event_tx.send(RunsEvent::Error(err));
                }
            },
            Ok(RunsCommand::LoadReplay {
                history_path,
                a_id,
                b_id,
                payoff,
            }) => match load_replay(&history_path, &a_id, &b_id, payoff) {
                Ok(replay) => {
                    let _ = event_tx.send(RunsEvent::ReplayLoaded(replay));
                }
                Err(err) => {
                    let _ = event_tx.send(RunsEvent::Error(err));
                }
            },
            Ok(RunsCommand::Shutdown) | Err(_) => break,
        }
    }
}

fn scan_runs(base_dir: &Path) -> Result<Vec<GamesRunEntry>, String> {
    let mut summaries = Vec::new();
    let runs_root = base_dir.join("runs").join("games");
    if runs_root.exists() {
        for entry in fs::read_dir(&runs_root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if path.file_name().and_then(|n| n.to_str()) == Some("sweeps") {
                if let Ok(sweeps) = fs::read_dir(&path) {
                    for sweep in sweeps.flatten() {
                        let sweep_path = sweep.path();
                        let cells = sweep_path.join("cells");
                        if let Ok(cell_dirs) = fs::read_dir(&cells) {
                            for cell in cell_dirs.flatten() {
                                let summary_path = cell.path().join("run_summary.json");
                                if summary_path.exists() {
                                    summaries.push(summary_path);
                                }
                            }
                        }
                    }
                }
                continue;
            }
            let summary_path = path.join("run_summary.json");
            if summary_path.exists() {
                summaries.push(summary_path);
            }
        }
    }

    for legacy_dir in [base_dir.join("games-runs"), base_dir.join("output")] {
        if !legacy_dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&legacy_dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_file() {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if name.starts_with("run__") && name.ends_with(".json") {
                    summaries.push(path);
                }
            }
        }
    }

    let mut entries = Vec::new();
    for summary_path in summaries {
        if let Ok(summary) = load_summary(&summary_path) {
            entries.push(entry_from_summary(&summary, &summary_path));
        }
    }
    entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(entries)
}

fn load_summary(path: &Path) -> Result<RunSummary, String> {
    let file = File::open(path).map_err(|e| format!("Failed to open {}: {e}", path.display()))?;
    serde_json::from_reader(file).map_err(|e| format!("Failed to parse {}: {e}", path.display()))
}

fn entry_from_summary(summary: &RunSummary, summary_path: &Path) -> GamesRunEntry {
    let run_dir = summary
        .run_dir
        .clone()
        .or_else(|| summary_path.parent().map(|p| p.display().to_string()));
    let label = format!(
        "{}  seed={}  run_id={}",
        summary.timestamp, summary.seed, summary.run_id
    );
    GamesRunEntry {
        label,
        summary_path: summary_path.display().to_string(),
        run_dir,
        seed: Some(summary.seed),
        timestamp: Some(summary.timestamp.clone()),
        run_id: Some(summary.run_id.clone()),
    }
}

fn load_replay(
    history_path: &Path,
    a_id: &str,
    b_id: &str,
    payoff: PayoffMatrix,
) -> Result<ReplayData, String> {
    let file = File::open(history_path)
        .map_err(|e| format!("Failed to open {}: {e}", history_path.display()))?;
    let reader = BufReader::new(file);
    let a_tag = format!("\"a\":\"{a_id}\"");
    let b_tag = format!("\"b\":\"{b_id}\"");
    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        if !line.contains(&a_tag) || !line.contains(&b_tag) {
            continue;
        }
        let record: MatchHistory =
            serde_json::from_str(&line).map_err(|e| format!("History parse error: {e}"))?;
        if record.a == a_id && record.b == b_id {
            return Ok(build_replay(record, payoff));
        }
    }
    Err(format!(
        "Match not found for {} vs {} in {}",
        a_id,
        b_id,
        history_path.display()
    ))
}

fn build_replay(record: MatchHistory, payoff: PayoffMatrix) -> ReplayData {
    let mut lines = Vec::new();
    let title = format!(
        "Match {} (id {})  {} vs {}  rep {}",
        record.match_index, record.match_id, record.a, record.b, record.repetition
    );
    lines.push(format!(
        "rounds: {}  score: {} - {}",
        record.resolved_rounds(),
        record.a_score,
        record.b_score
    ));
    if let Some(cycle) = record.cycle.clone() {
        let cycle_start = cycle.transient_rounds.saturating_add(1);
        lines.push(format!(
            "cycle: start={} len={} a_coop={:.3} b_coop={:.3}",
            cycle_start, cycle.cycle_rounds, cycle.a_cycle_coop_rate, cycle.b_cycle_coop_rate
        ));
    }
    lines.push("".into());
    lines.push("round  a  b  outcome  payoff".into());

    let scores = record.score_idx.chars().collect::<Vec<_>>();
    for (idx, outcome) in scores.into_iter().enumerate() {
        let Some((a_action, b_action)) = actions_from_outcome(outcome) else {
            continue;
        };
        let a_char = a_action.as_char();
        let b_char = b_action.as_char();
        let (a_pay, b_pay) = payoff.payoffs(a_action, b_action);
        lines.push(format!(
            "{:>4}  {}  {}   {}      {:>2} {:>2}",
            idx + 1,
            a_char,
            b_char,
            outcome_label(outcome),
            a_pay,
            b_pay
        ));
    }

    ReplayData {
        title,
        lines,
        cycle: record.cycle,
    }
}

fn actions_from_outcome(ch: char) -> Option<(Action, Action)> {
    match ch {
        '0' => Some((Action::Cooperate, Action::Cooperate)),
        '1' => Some((Action::Cooperate, Action::Defect)),
        '2' => Some((Action::Defect, Action::Cooperate)),
        '3' => Some((Action::Defect, Action::Defect)),
        _ => None,
    }
}

#[cfg(test)]
#[path = "tests/games_runs.rs"]
mod tests;

fn outcome_label(ch: char) -> &'static str {
    match ch {
        '0' => "CC",
        '1' => "CD",
        '2' => "DC",
        '3' => "DD",
        _ => "--",
    }
}
