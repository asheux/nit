//! Background genome evaluation worker.
//!
//! Moves all `compute_genome_report` calls off the main thread so the UI
//! never blocks on GoL simulation. Files are evaluated on short-lived
//! threads; results stream back through a channel that the main loop drains.

use nit_core::genome_report::GenomeReport;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

/// Result of a background genome evaluation.
pub struct GenomeEvalResult {
    pub path: PathBuf,
    pub report: GenomeReport,
    /// `true` for shadow evaluations (during a turn), `false` for
    /// authoritative turn-completion evaluations.
    pub shadow: bool,
}

/// Handle held by the main thread. Send files for evaluation, drain results.
pub struct GenomeWorker {
    tx: Sender<GenomeEvalResult>,
    /// Results channel — drain this every frame with `try_recv`.
    pub rx: Receiver<GenomeEvalResult>,
}

/// Stack size for genome evaluation threads (2 MB — sufficient for
/// tree-sitter parsing + GoL simulation, avoids 8 MB default waste).
const EVAL_STACK_SIZE: usize = 2 * 1024 * 1024;

impl Default for GenomeWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl GenomeWorker {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx }
    }

    /// Spawn a background thread to evaluate a single file.
    /// The result is sent to `self.rx` when ready.
    pub fn evaluate(&self, path: PathBuf, text: String, shadow: bool) {
        let tx = self.tx.clone();
        let _ = std::thread::Builder::new()
            .name("genome-eval".into())
            .stack_size(EVAL_STACK_SIZE)
            .spawn(move || {
                let report = nit_core::compute_genome_report(&text, &path);
                let _ = tx.send(GenomeEvalResult {
                    path,
                    report,
                    shadow,
                });
            });
    }
}
