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
    engine: &TournamentKernel,
    event_path: Option<PathBuf>,
    history_path: Option<PathBuf>,
) -> anyhow::Result<TournamentRun> {
    let cfg = engine.config();
    let parallelism = Parallelism::from_config(&cfg.engine.parallelism);

    if matches!(parallelism, Parallelism::Off) {
        run_sequential(engine, cfg, event_path, history_path)
    } else {
        run_parallel(engine, cfg, parallelism, event_path, history_path)
    }
}

fn run_sequential(
    engine: &TournamentKernel,
    cfg: &NormalizedConfig,
    event_file: Option<PathBuf>,
    history_file: Option<PathBuf>,
) -> anyhow::Result<TournamentRun> {
    let mut events = event_file
        .map(|p| EventWriter::new(p, cfg.event_log.include_rounds))
        .transpose()?;
    let mut history = history_file.map(HistoryWriter::new).transpose()?;

    let (results, runtime) = engine.run_with_runtime(KernelRunMode::Sequential {
        event_writer: events.as_mut(),
        history_writer: history.as_mut(),
    });

    let event_log_path = match events {
        Some(w) => Some(
            w.finish()
                .with_context(|| "failed to finalize event log")?
                .to_string_lossy()
                .to_string(),
        ),
        None => None,
    };
    let history_log_path = match history {
        Some(w) => Some(
            w.finish()
                .with_context(|| "failed to finalize history log")?
                .to_string_lossy()
                .to_string(),
        ),
        None => None,
    };

    Ok(TournamentRun {
        results,
        runtime,
        event_log_path,
        history_log_path,
    })
}

fn run_parallel(
    engine: &TournamentKernel,
    cfg: &NormalizedConfig,
    parallelism: Parallelism,
    event_file: Option<PathBuf>,
    history_file: Option<PathBuf>,
) -> anyhow::Result<TournamentRun> {
    let event_writer = event_file
        .map(|p| EventWriter::new(p, cfg.event_log.include_rounds))
        .transpose()?;
    let (event_sender, event_thread) = spawn_writer_thread(event_writer);

    let history_writer = history_file.map(HistoryWriter::new).transpose()?;
    let (history_sender, history_thread) = spawn_writer_thread(history_writer);

    let (results, runtime) = engine.run_with_runtime(KernelRunMode::Parallel {
        parallelism,
        event_sender: event_sender.clone(),
        include_rounds: cfg.event_log.include_rounds,
        history_sender: history_sender.clone(),
    });

    // Drop senders before joining so the writer threads observe channel close.
    drop(event_sender);
    drop(history_sender);

    let event_log_path = match event_thread {
        Some(h) => Some(
            h.join()
                .map_err(|_| anyhow::anyhow!("event log worker panicked"))?
                .with_context(|| "failed to finalize event log")?
                .to_string_lossy()
                .to_string(),
        ),
        None => None,
    };
    let history_log_path = match history_thread {
        Some(h) => Some(
            h.join()
                .map_err(|_| anyhow::anyhow!("history log worker panicked"))?
                .with_context(|| "failed to finalize history log")?
                .to_string_lossy()
                .to_string(),
        ),
        None => None,
    };

    Ok(TournamentRun {
        results,
        runtime,
        event_log_path,
        history_log_path,
    })
}

type WriterHandle<T> = (
    Option<mpsc::Sender<T>>,
    Option<thread::JoinHandle<std::io::Result<PathBuf>>>,
);

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

// Unified interface for event and history writers, enabling generic
// background threading and sequential finalization.
trait RecordSink: Send + 'static {
    type Record: Send + 'static;
    fn accept(&mut self, record: &Self::Record) -> std::io::Result<()>;
    fn finish(self) -> std::io::Result<PathBuf>;
}

macro_rules! impl_record_sink {
    ($writer:ty, $record:ty) => {
        impl RecordSink for $writer {
            type Record = $record;
            fn accept(&mut self, record: &$record) -> std::io::Result<()> {
                self.write(record)
            }
            fn finish(self) -> std::io::Result<PathBuf> {
                <$writer>::finish(self)
            }
        }
    };
}

impl_record_sink!(EventWriter, GameEvent);
impl_record_sink!(HistoryWriter, MatchHistory);
