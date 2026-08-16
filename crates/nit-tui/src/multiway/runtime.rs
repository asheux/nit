//! Phase 3 + 4 wiring: [`MultiwayRuntime`] runs the pure `nit-multiway` [`Engine`]
//! over the real `GitWorldStore` + genome/gates [`GenomeValuer`] + the
//! spawned-agent [`RunnerExecutor`], with Phase 4 merge enabled: a
//! [`JudgeMergeOracle`] adjudicates conflicting joins. The executor and the oracle
//! share one [`RunnerExecutor`] via `Arc` — so one `claude` runner and one event
//! channel back both — which is sound because v1 expands serially (one in-flight
//! turn ever). All behind the once-resolved `NIT_MULTIWAY` flag.
//!
//! Off-path discipline (mirrors `NIT_CLAUDE_POOL=0` / `NIT_PLANNER_LEGACY=1`):
//! the flag is read ONCE in [`MultiwayRuntime::from_env`] and cached, so a
//! mid-mission env change can't flip behaviour. With the flag off the caller
//! never reaches [`MultiwayRuntime::start`], so the existing chat-dispatch path
//! is byte-identical to today.
//!
//! Each search runs on its own thread, off the UI thread. That thread OWNS the
//! `GitWorldStore`; when it exits — completion, abort, or app quit — the store's
//! `Drop::cleanup` removes every worktree and prunes the mission's refs. The
//! store is never parked in a retained collection, which is what would leak.
//!
//! [`Engine`]: nit_multiway::search::Engine
//! [`GenomeValuer`]: crate::multiway::valuer::GenomeValuer
//! [`RunnerExecutor`]: crate::multiway::executor::RunnerExecutor

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use nit_core::{GenomeTier, MultiwayView};
use nit_multiway::frontier::Frontier;
use nit_multiway::git_store::GitWorldStore;
use nit_multiway::graph::Graph;
use nit_multiway::policy::SearchPolicy;
use nit_multiway::search::{Engine, RunSnapshot, SearchOutcome};
use nit_multiway::traits::Task;

use crate::claude_runner::ClaudeRunner;
use crate::multiway::build_multiway_view;
use crate::multiway::executor::{JudgeMergeOracle, RunnerExecutor, SharedExecutor};
use crate::multiway::valuer::GenomeValuer;
use crate::swarm::effective_max_swarm_size;

const NIT_MULTIWAY_ENV: &str = "NIT_MULTIWAY";
const NIT_MULTIWAY_GATES_ENV: &str = "NIT_MULTIWAY_GATES";

/// Status updates drained by the caller's UI tick via [`MultiwayRuntime::poll`].
/// `Progress` is the scalar status-line heartbeat and `View` the full Phase 6b
/// popup snapshot (one per expansion); the two consumers stay decoupled. `Done`/
/// `Failed` are terminal and arrive only after the worktrees/refs have already
/// been cleaned up.
#[derive(Clone, Debug)]
pub enum MultiwayEvent {
    Progress {
        nodes: usize,
        turns: usize,
        frontier: usize,
        best_score: f32,
        best_tier: GenomeTier,
    },
    /// A full search snapshot streamed after each expansion for the live popup,
    /// drained where `drive_multiway` already polls the channel.
    View(MultiwayView),
    Done(SearchOutcome),
    Failed(String),
}

#[derive(Debug)]
pub enum RunError {
    /// A search for this mission id is already in flight.
    AlreadyRunning,
    /// The OS refused to spawn the search thread.
    SpawnFailed(std::io::Error),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::AlreadyRunning => {
                f.write_str("a multiway search is already running for this mission")
            }
            RunError::SpawnFailed(e) => {
                write!(f, "could not spawn the multiway search thread: {e}")
            }
        }
    }
}

impl std::error::Error for RunError {}

/// One in-flight search. The `cancel` flag is shared with the search thread's
/// executor; flipping it aborts the turn. `finished` is set by the thread only
/// after its store has dropped, so observing it true means cleanup has run.
struct ActiveRun {
    cancel: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    events: Receiver<MultiwayEvent>,
    handle: Option<JoinHandle<()>>,
}

pub struct MultiwayRuntime {
    enabled: bool,
    /// Gate-command override resolved ONCE from `NIT_MULTIWAY_GATES` at
    /// construction (mirrors `enabled`), so a mid-mission env change can't flip
    /// valuation. `None` keeps language auto-detection; `Some` — including the
    /// empty genome-only set — overrides it. See [`resolve_gate_override`].
    gate_override: Option<Vec<String>>,
    runs: HashMap<String, ActiveRun>,
}

impl MultiwayRuntime {
    pub fn from_env() -> Self {
        Self {
            enabled: multiway_flag_enabled(std::env::var(NIT_MULTIWAY_ENV).ok().as_deref()),
            gate_override: resolve_gate_override(
                std::env::var(NIT_MULTIWAY_GATES_ENV).ok().as_deref(),
            ),
            runs: HashMap::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_active(&self, mission_id: &str) -> bool {
        self.runs
            .get(mission_id)
            .is_some_and(|run| !run.finished.load(Ordering::SeqCst))
    }

    /// Spawn the search on its own thread. `policy.k` is clamped to the
    /// FD-bounded swarm ceiling here, snapshotted once, so a host with a tight
    /// `ulimit -n` can't be driven into `EMFILE` by an over-wide fork.
    pub fn start(
        &mut self,
        mission_id: &str,
        operator_cwd: PathBuf,
        runner: ClaudeRunner,
        model: String,
        task: Task,
        policy: SearchPolicy,
    ) -> Result<(), RunError> {
        if self.is_active(mission_id) {
            return Err(RunError::AlreadyRunning);
        }
        // Reap a previously-finished run for this id; its slot is free to reuse.
        self.runs.remove(mission_id);

        let mut policy = policy;
        policy.k = clamp_fork_width(policy.k, effective_max_swarm_size());

        let cancel = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let (tx, events) = mpsc::channel();

        let mission = mission_id.to_owned();
        let thread_cancel = Arc::clone(&cancel);
        let thread_finished = Arc::clone(&finished);
        let gate_override = self.gate_override.clone();
        let handle = thread::Builder::new()
            .name(format!("nit-multiway-{mission}"))
            .spawn(move || {
                let outcome = run_search(
                    &operator_cwd,
                    &mission,
                    runner,
                    model,
                    task,
                    policy,
                    gate_override,
                    &thread_cancel,
                    &tx,
                );
                // `run_search` already dropped the store (and ran its cleanup)
                // before returning, so the terminal event signals a clean tree.
                let _ = match outcome {
                    Ok(found) => tx.send(MultiwayEvent::Done(found)),
                    Err(err) => tx.send(MultiwayEvent::Failed(err)),
                };
                thread_finished.store(true, Ordering::SeqCst);
            })
            .map_err(RunError::SpawnFailed)?;

        self.runs.insert(
            mission_id.to_owned(),
            ActiveRun {
                cancel,
                finished,
                events,
                handle: Some(handle),
            },
        );
        Ok(())
    }

    pub fn poll(&mut self, mission_id: &str) -> Vec<MultiwayEvent> {
        let mut drained = Vec::new();
        if let Some(run) = self.runs.get(mission_id) {
            while let Ok(event) = run.events.try_recv() {
                drained.push(event);
            }
        }
        drained
    }

    pub fn abort(&mut self, mission_id: &str) {
        if let Some(run) = self.runs.get(mission_id) {
            run.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn abort_all(&mut self) {
        for run in self.runs.values() {
            run.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Join the threads of runs that have signalled `finished`, dropping their
    /// records. Call on the UI tick / quit so completed searches don't linger.
    pub fn cleanup_finished(&mut self) {
        let done: Vec<String> = self
            .runs
            .iter()
            .filter(|(_, run)| run.finished.load(Ordering::SeqCst))
            .map(|(id, _)| id.clone())
            .collect();
        for id in done {
            if let Some(mut run) = self.runs.remove(&id) {
                if let Some(handle) = run.handle.take() {
                    let _ = handle.join();
                }
            }
        }
    }
}

impl Drop for MultiwayRuntime {
    fn drop(&mut self) {
        // Quit must not leak worktrees: signal every search, then block until
        // each thread exits so its store's `Drop::cleanup` completes.
        for run in self.runs.values() {
            run.cancel.store(true, Ordering::Relaxed);
        }
        for run in self.runs.values_mut() {
            if let Some(handle) = run.handle.take() {
                let _ = handle.join();
            }
        }
    }
}

/// `true` only for an explicit affirmative (`1`/`true`/`yes`/`on`), matching the
/// `NIT_PLANNER_LEGACY` / `NIT_CLAUDE_POOL` opt-in convention; anything else —
/// unset included — keeps the engine off and the dispatch path byte-identical.
pub fn multiway_flag_enabled(raw: Option<&str>) -> bool {
    matches!(
        raw.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// Clamp the fork fan-out into the FD-bounded swarm ceiling. Each live
/// worktree+turn holds open fds, so an unclamped `k` could exhaust the limit and
/// crash the TUI; `max(1)` keeps the search runnable on a degenerate ceiling.
pub fn clamp_fork_width(k: usize, cap: usize) -> usize {
    k.clamp(1, cap.max(1))
}

/// Resolve the `NIT_MULTIWAY_GATES` override into a gate-command set, ONCE at
/// runtime construction (mirrors [`multiway_flag_enabled`]). Tri-state:
/// - unset (`None`) -> `None`: auto-detect the language gate bundle.
/// - empty (`Some("")`) -> `Some(vec![])`: genome-only, the operator fail-open.
/// - `"a ; b"` -> `Some(vec!["a", "b"])`: run exactly these, all-must-pass.
///
/// Entries split on `;` and are trimmed; blanks drop, so `";"` and `"   "` both
/// collapse to the genome-only set, never to `None`. Each entry stays one string
/// — the argv whitespace-split (no shell) happens later at spawn time. The
/// `raw.map(..)` over `Option<&str>` is load-bearing: `.ok().filter(..)` would
/// fold the empty string into `None` and silently kill the documented fail-open.
pub fn resolve_gate_override(raw: Option<&str>) -> Option<Vec<String>> {
    raw.map(|spec| {
        spec.split(';')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_owned)
            .collect()
    })
}

/// Pick the node valuer from the resolved gate override. An explicit set —
/// including the empty genome-only set — is used verbatim via [`GenomeValuer::new`];
/// `None` falls back to language auto-detection on the operator tree, byte-identical
/// to the pre-override path.
pub fn select_valuer(gate_override: Option<Vec<String>>, operator_cwd: &Path) -> GenomeValuer {
    match gate_override {
        Some(commands) => GenomeValuer::new(commands),
        None => GenomeValuer::for_tree(operator_cwd),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_search(
    operator_cwd: &Path,
    mission: &str,
    runner: ClaudeRunner,
    model: String,
    task: Task,
    policy: SearchPolicy,
    gate_override: Option<Vec<String>>,
    cancel: &Arc<AtomicBool>,
    tx: &Sender<MultiwayEvent>,
) -> Result<SearchOutcome, String> {
    let store = GitWorldStore::for_mission(operator_cwd, mission).map_err(|e| e.to_string())?;
    let valuer = select_valuer(gate_override, operator_cwd);
    let executor = SharedExecutor::new(RunnerExecutor::new(
        runner,
        model,
        mission.to_owned(),
        Arc::clone(cancel),
    ));
    let oracle = JudgeMergeOracle::new(executor.clone());
    let mut engine = Engine::new(store, executor, valuer, oracle);

    let root = engine.seed(operator_cwd).map_err(|e| e.to_string())?;
    // Phase 6b live feed: stream a full view snapshot after every expansion. The
    // closure captures only the `Sender` and `policy` — never the store, engine,
    // or a worktree — so the store stays owned by this thread and its `Drop`
    // cleanup runs exactly as before.
    let mut observer = |graph: &Graph, frontier: &Frontier, snap: &RunSnapshot| {
        let _ = tx.send(MultiwayEvent::View(build_multiway_view(
            graph, frontier, snap, &policy, None,
        )));
    };
    let search = engine
        .run_merging_observed(root, &policy, &task, &mut observer)
        .map_err(|e| e.to_string());

    // Persist whatever DAG was built even on a failed/aborted run — a partial
    // DAG is still the record of what the search explored.
    let persisted = persist_dag(mission, &engine.graph);

    let outcome = search?;
    persisted?;
    // The observer only ever fires mid-search (stop_reason None), so without this
    // the popup header reads "searching…" forever after the loop ends. Stream one
    // final view carrying the terminal stop reason; best_score is read off the
    // winning node the same way `progress_snapshot` does.
    let final_snapshot = RunSnapshot {
        turns_spent: outcome.turns_spent,
        nodes_expanded: outcome.nodes_expanded,
        best: outcome.best.clone(),
        best_score: outcome
            .best
            .as_ref()
            .and_then(|id| engine.graph.node(id))
            .and_then(|node| node.value.as_ref())
            .map(|value| value.score)
            .unwrap_or(0.0),
    };
    let _ = tx.send(MultiwayEvent::View(build_multiway_view(
        &engine.graph,
        &engine.frontier,
        &final_snapshot,
        &policy,
        Some(outcome.stop_reason),
    )));
    let _ = tx.send(progress_snapshot(
        &engine.graph,
        engine.frontier.len(),
        &outcome,
    ));
    Ok(outcome)
    // `engine` (and its `GitWorldStore`) drop here -> worktrees + refs cleaned.
}

/// Persist the DAG to `<state_dir>/multiway/<mission>.json` via the engine's
/// atomic [`Graph::save`] (temp file + rename). The mission id is reduced to a
/// filename-safe leaf so it can never escape the multiway dir.
fn persist_dag(mission: &str, graph: &Graph) -> Result<(), String> {
    let base = nit_utils::paths::state_dir()
        .or_else(nit_utils::paths::data_dir)
        .ok_or_else(|| "no state or data directory for DAG persistence".to_owned())?;
    let leaf: String = mission
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let leaf = if leaf.is_empty() {
        "mission"
    } else {
        leaf.as_str()
    };
    let path = base.join("multiway").join(format!("{leaf}.json"));
    graph
        .save(&path)
        .map_err(|e| format!("persist DAG to {}: {e}", path.display()))
}

/// The steer shown when a finished search kept no viable node. `uv run pytest -q`
/// is the canonical clean-worktree gate; the trailing `NIT_MULTIWAY_GATES=` form is
/// the genome-only escape. Kept as a `const` so the terminal renderer and the tests
/// share one source of truth for the wording.
pub const ALL_GATED_HINT: &str = "all candidates gate-failed; set NIT_MULTIWAY_GATES to a command that runs in a clean worktree (e.g. uv run pytest -q), or NIT_MULTIWAY_GATES= for genome-only.";

/// Operator hint for a finished search whose every candidate failed its gates:
/// `outcome.best` is `None` because gated children never commit, so they are never
/// observed as a best node. `None` on a normal finish, where the caller renders its
/// own summary instead.
pub fn all_gated_hint(outcome: &SearchOutcome) -> Option<&'static str> {
    outcome.best.is_none().then_some(ALL_GATED_HINT)
}

fn progress_snapshot(graph: &Graph, frontier: usize, outcome: &SearchOutcome) -> MultiwayEvent {
    let (best_score, best_tier) = outcome
        .best
        .as_ref()
        .and_then(|id| graph.node(id))
        .and_then(|node| node.value.as_ref())
        .map(|value| (value.score, value.tier))
        .unwrap_or((0.0, GenomeTier::StillLife));
    MultiwayEvent::Progress {
        nodes: graph.len(),
        turns: outcome.turns_spent,
        frontier,
        best_score,
        best_tier,
    }
}
