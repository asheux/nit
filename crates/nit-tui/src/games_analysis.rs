use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use nit_games::analysis::{analyze_history, AnalysisConfig, HistoryAnalysis};

/// Parameters describing a single history-analysis job dispatched to the runner thread.
#[derive(Clone, Debug)]
pub struct AnalysisRequest {
    pub history_path: PathBuf,
    pub out_dir: PathBuf,
    pub tail_rounds: usize,
    pub trajectory_samples: usize,
}

pub enum AnalysisCommand {
    Analyze(AnalysisRequest),
    Shutdown,
}

pub enum AnalysisEvent {
    Started(Box<AnalysisRequest>),
    Finished(Box<HistoryAnalysis>),
    Error(String),
}

/// Background thread that serialises expensive history-analysis jobs off the UI loop.
pub struct GamesAnalysisRunner {
    cmd_tx: Sender<AnalysisCommand>,
    pub events: Receiver<AnalysisEvent>,
    handle: Option<JoinHandle<()>>,
}

impl GamesAnalysisRunner {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("nit-games-analysis".into())
            .spawn(move || runner_loop(cmd_rx, event_tx))
            .expect("spawn games analysis runner");
        Self {
            cmd_tx,
            events: event_rx,
            handle: Some(handle),
        }
    }

    pub fn send(&self, command: AnalysisCommand) {
        let _ = self.cmd_tx.send(command);
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(AnalysisCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn runner_loop(cmd_rx: Receiver<AnalysisCommand>, event_tx: Sender<AnalysisEvent>) {
    while let Ok(command) = cmd_rx.recv() {
        match command {
            AnalysisCommand::Analyze(request) => run_analysis(request, &event_tx),
            AnalysisCommand::Shutdown => break,
        }
    }
}

fn run_analysis(request: AnalysisRequest, event_tx: &Sender<AnalysisEvent>) {
    let _ = event_tx.send(AnalysisEvent::Started(Box::new(request.clone())));
    let config = AnalysisConfig {
        tail_rounds: request.tail_rounds,
        trajectory_samples: request.trajectory_samples,
        ..AnalysisConfig::default()
    };
    let event = match analyze_history(&request.history_path, &request.out_dir, config) {
        Ok(result) => AnalysisEvent::Finished(Box::new(result)),
        Err(err) => AnalysisEvent::Error(err),
    };
    let _ = event_tx.send(event);
}
