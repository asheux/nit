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
    pub editor_open: bool,
    /// Launch / file-watcher triggered background scan: populates
    /// `genome_reports` for every source file in the workspace without being
    /// tied to an agent or turn. Set by `evaluate_from_disk_workspace_scan`.
    pub workspace_scan: bool,
    /// Routes authoritative results back to the correct in-flight batch so
    /// parallel swarm turns finalize independently. `None` for shadow/save/
    /// editor-opened evals.
    pub agent_id: Option<String>,
}

enum DiskEval {
    Save,
    Shadow,
    Workspace,
    Turn(String),
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

    pub fn evaluate_editor(&self, path: PathBuf, text: String) -> bool {
        let tx = self.tx.clone();
        eval_thread()
            .spawn(move || {
                let report = nit_core::compute_genome_report(&text, &path);
                let _ = tx.send(GenomeEvalResult {
                    path,
                    report: Some(report),
                    shadow: false,
                    save_eval: false,
                    editor_open: true,
                    workspace_scan: false,
                    agent_id: None,
                });
            })
            .is_ok()
    }

    pub fn evaluate_saved_file(&self, path: PathBuf) -> bool {
        self.evaluate_from_disk_inner(path, DiskEval::Save)
    }

    /// Authoritative turn-completion eval: the worker reads the file itself so
    /// the caller does not have to buffer the contents. Returns `false` when
    /// the thread could not be spawned; the caller must then decrement
    /// pending counters manually.
    pub fn evaluate_from_disk(&self, path: PathBuf, agent_id: String) -> bool {
        self.evaluate_from_disk_inner(path, DiskEval::Turn(agent_id))
    }

    pub fn evaluate_from_disk_shadow(&self, path: PathBuf) -> bool {
        self.evaluate_from_disk_inner(path, DiskEval::Shadow)
    }

    /// Background workspace-scan eval: inserts into `state.genome_reports`
    /// without any shadow/batch/agent bookkeeping. Used by the launch scan
    /// and the file-watcher invalidation path so every source file has a
    /// report ready the moment an agent needs the landscape.
    pub fn evaluate_from_disk_workspace_scan(&self, path: PathBuf) -> bool {
        self.evaluate_from_disk_inner(path, DiskEval::Workspace)
    }

    fn evaluate_from_disk_inner(&self, path: PathBuf, kind: DiskEval) -> bool {
        let tx = self.tx.clone();
        eval_thread()
            .spawn(move || {
                let report = nit_core::io::load_to_string(&path)
                    .ok()
                    .map(|text| nit_core::compute_genome_report(&text, &path));
                let _ = tx.send(GenomeEvalResult {
                    path,
                    report,
                    editor_open: false,
                    shadow: matches!(kind, DiskEval::Shadow),
                    save_eval: matches!(kind, DiskEval::Save),
                    workspace_scan: matches!(kind, DiskEval::Workspace),
                    agent_id: match kind {
                        DiskEval::Turn(id) => Some(id),
                        _ => None,
                    },
                });
            })
            .is_ok()
    }
}

fn eval_thread() -> std::thread::Builder {
    std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .stack_size(EVAL_STACK_SIZE)
}
