use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use anyhow::Context;
use nit_games::{
    events::{EventWriter, GameEvent},
    history_log::MatchHistory,
    output::TournamentResults,
    tournament::{KernelRunMode, Parallelism, TournamentKernel},
    HistoryWriter, NormalizedConfig, RuntimeAcceleratorStats,
};

pub(super) struct TournamentRun {
    pub results: TournamentResults,
    /// GPU utilization and kernel timing metrics.
    pub runtime: RuntimeAcceleratorStats,
    pub event_log_path: Option<String>,
    pub history_log_path: Option<String>,
}

pub(super) fn execute_tournament(
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

    // Drop senders before joining so the writer threads can observe channel close.
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
