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
    /// `None` when the file could not be read (e.g. deleted before the worker
    /// ran).  The main loop should still decrement pending counters.
    pub report: Option<GenomeReport>,
    /// `true` for shadow evaluations (during a turn), `false` for
    /// authoritative turn-completion evaluations.
    pub shadow: bool,
    /// `true` when this evaluation was triggered by a manual file save.
    pub save_eval: bool,
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
        self.evaluate_inner(path, text, shadow, false);
    }

    /// Like `evaluate`, but marks the result as originating from a manual save.
    pub fn evaluate_save(&self, path: PathBuf, text: String) {
        self.evaluate_inner(path, text, false, true);
    }

    /// Like `evaluate`, but reads the file from disk on the worker thread
    /// instead of requiring the caller to pass the text.  Returns `false` if
    /// the thread could not be spawned (the caller should decrement pending
    /// counts in that case).
    /// Like `evaluate`, but reads the file from disk on the worker thread
    /// instead of requiring the caller to pass the text.  Returns `false` if
    /// the thread could not be spawned (the caller should decrement pending
    /// counts in that case).  If the file read fails on the worker, a result
    /// with `report: None` is sent so pending counts still decrement.
    pub fn evaluate_from_disk(&self, path: PathBuf) -> bool {
        self.evaluate_from_disk_inner(path, false)
    }

    /// Shadow variant of `evaluate_from_disk`.
    pub fn evaluate_from_disk_shadow(&self, path: PathBuf) -> bool {
        self.evaluate_from_disk_inner(path, true)
    }

    fn evaluate_from_disk_inner(&self, path: PathBuf, shadow: bool) -> bool {
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("genome-eval".into())
            .stack_size(EVAL_STACK_SIZE)
            .spawn(move || {
                let report = std::fs::read_to_string(&path)
                    .ok()
                    .map(|text| nit_core::compute_genome_report(&text, &path));
                let _ = tx.send(GenomeEvalResult {
                    path,
                    report,
                    shadow,
                    save_eval: false,
                });
            })
            .is_ok()
    }

    fn evaluate_inner(&self, path: PathBuf, text: String, shadow: bool, save_eval: bool) {
        let tx = self.tx.clone();
        let _ = std::thread::Builder::new()
            .name("genome-eval".into())
            .stack_size(EVAL_STACK_SIZE)
            .spawn(move || {
                let report = nit_core::compute_genome_report(&text, &path);
                let _ = tx.send(GenomeEvalResult {
                    path,
                    report: Some(report),
                    shadow,
                    save_eval,
                });
            });
    }
}
