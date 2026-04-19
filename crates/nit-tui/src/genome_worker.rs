//! Runs `compute_genome_report` on short-lived worker threads so the UI
//! never blocks on tree-sitter parsing or GoL simulation.

use nit_core::genome_report::GenomeReport;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

pub struct GenomeEvalResult {
    pub path: PathBuf,
    /// `None` when the file read failed (e.g. deleted before the worker ran).
    /// The main loop must still decrement pending counters in that case.
    pub report: Option<GenomeReport>,
    /// `true` for shadow evaluations during a turn; `false` for authoritative
    /// turn-completion evaluations.
    pub shadow: bool,
    pub save_eval: bool,
    /// Proposer pre-scan: populates `genome_reports` for scope files before
    /// propose-role dispatch, so the proposer sees a real landscape instead
    /// of empty data on fresh workspaces. Not tied to any agent or batch.
    pub prescan: bool,
    /// Routes authoritative results back to the correct in-flight batch so
    /// parallel swarm turns finalize independently. `None` for shadow/save/
    /// editor-opened evals.
    pub agent_id: Option<String>,
}

pub struct GenomeWorker {
    tx: Sender<GenomeEvalResult>,
    pub rx: Receiver<GenomeEvalResult>,
}

// 2 MB matches tree-sitter + GoL worst case; the 8 MB default wastes address
// space when many evals are in flight at once.
const EVAL_STACK_SIZE: usize = 2 * 1024 * 1024;
const THREAD_NAME: &str = "genome-eval";

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

    pub fn evaluate(&self, path: PathBuf, text: String, shadow: bool) {
        self.evaluate_inner(path, text, shadow, false);
    }

    pub fn evaluate_save(&self, path: PathBuf, text: String) {
        self.evaluate_inner(path, text, false, true);
    }

    /// Authoritative turn-completion eval: the worker reads the file itself so
    /// the caller does not have to buffer the contents. Returns `false` when
    /// the thread could not be spawned; the caller must then decrement
    /// pending counters manually.
    pub fn evaluate_from_disk(&self, path: PathBuf, agent_id: String) -> bool {
        self.evaluate_from_disk_inner(path, false, false, Some(agent_id))
    }

    pub fn evaluate_from_disk_shadow(&self, path: PathBuf) -> bool {
        self.evaluate_from_disk_inner(path, true, false, None)
    }

    /// Proposer pre-scan eval: inserts into `state.genome_reports` without
    /// the shadow/batch bookkeeping so a follow-up propose-role dispatch
    /// sees real landscape data even on a fresh workspace.
    pub fn evaluate_from_disk_prescan(&self, path: PathBuf) -> bool {
        self.evaluate_from_disk_inner(path, false, true, None)
    }

    fn evaluate_from_disk_inner(
        &self,
        path: PathBuf,
        shadow: bool,
        prescan: bool,
        agent_id: Option<String>,
    ) -> bool {
        let tx = self.tx.clone();
        eval_thread()
            .spawn(move || {
                let report = std::fs::read_to_string(&path)
                    .ok()
                    .map(|text| nit_core::compute_genome_report(&text, &path));
                let _ = tx.send(GenomeEvalResult {
                    path,
                    report,
                    shadow,
                    save_eval: false,
                    prescan,
                    agent_id,
                });
            })
            .is_ok()
    }

    fn evaluate_inner(&self, path: PathBuf, text: String, shadow: bool, save_eval: bool) {
        let tx = self.tx.clone();
        let _ = eval_thread().spawn(move || {
            let report = nit_core::compute_genome_report(&text, &path);
            let _ = tx.send(GenomeEvalResult {
                path,
                report: Some(report),
                shadow,
                save_eval,
                prescan: false,
                agent_id: None,
            });
        });
    }
}

fn eval_thread() -> std::thread::Builder {
    std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .stack_size(EVAL_STACK_SIZE)
}
