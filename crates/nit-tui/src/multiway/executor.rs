//! The real [`TurnExecutor`]: it drives one speculative agent turn inside an
//! isolated git worktree by reusing the existing `claude_runner` `RunTurn` path,
//! rather than reimplementing subprocess handling. The engine forks `k`
//! worktrees and calls [`RunnerExecutor::run_turn`] on each; this adapter is the
//! seam between the pure search loop and a spawned `claude` process.
//!
//! Two invariants are load-bearing (correctness, not style):
//! - **Worktree-only.** The turn runs with `cwd = worktree`, never the operator's
//!   primary tree — the live editor auto-saves over that tree, so a turn there
//!   would corrupt an open buffer. The engine isolates exactly to prevent this.
//! - **Dead branch ≠ engine abort.** A failed turn is a normal search event
//!   ([`TurnStatus::Failed`]), so it maps to a `TurnResult`, not an `Err`. Only an
//!   operator cancel (or the runner channel dropping) propagates as an error and
//!   stops the whole search.
//!
//! The module also hosts the [`MergeOracle`] implementations: [`NoMergeOracle`],
//! the Phase 2/3 placeholder, and [`JudgeMergeOracle`], the Phase 4 oracle that
//! adjudicates a conflicted merge by driving one write-capable judge turn — the
//! same two invariants apply (it runs with `cwd = a`, and a judge that fails to
//! resolve is a dead merge branch the valuer gates, never an engine abort).

use std::error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::Duration;

use nit_core::{AgentBusEvent, OPERATOR_CANCEL_TURN_MESSAGE};
use nit_multiway::traits::{
    Conflicts, MergeOracle, ResolvedState, Task, TurnExecutor, TurnResult, TurnStatus,
};

use crate::claude_runner::{ClaudeCommand, ClaudeRunner, INTEGRATOR_MAX_TURNS};

/// Poll cadence on the event channel. Short enough that an operator cancel that
/// flips the shared flag is observed promptly even while the turn streams no
/// events; the channel `recv` does the real blocking.
const EVENT_POLL: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub enum ExecutorError {
    /// The operator aborted the mission (flag flipped or the runner reported the
    /// cancel sentinel). Propagates up `?` to stop the search and tear down.
    Cancelled,
    /// The owned runner's event channel disconnected before a terminal event —
    /// the runner thread shut down or crashed, so no result is recoverable.
    RunnerGone,
    /// [`NoMergeOracle`] was asked to adjudicate; merge is a Phase 4 feature and
    /// the Phase 2/3 search loop never triggers it.
    MergeUnsupported,
}

impl fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            ExecutorError::Cancelled => "multiway turn cancelled",
            ExecutorError::RunnerGone => {
                "multiway runner channel disconnected before the turn ended"
            }
            ExecutorError::MergeUnsupported => {
                "merge adjudication is unsupported (deferred to Phase 4)"
            }
        };
        f.write_str(msg)
    }
}

impl error::Error for ExecutorError {}

/// Drives turns through a dedicated [`ClaudeRunner`] this executor OWNS, so the
/// runner's event channel is private and never competes with the app's runner.
/// Because the engine expands one worktree at a time (serial v1), a single turn
/// is ever in flight and the events on the channel all belong to it.
pub struct RunnerExecutor {
    runner: ClaudeRunner,
    model: String,
    mission_id: String,
    seq: AtomicU64,
    cancel: Arc<AtomicBool>,
}

impl RunnerExecutor {
    pub fn new(
        runner: ClaudeRunner,
        model: String,
        mission_id: String,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        Self {
            runner,
            model,
            mission_id,
            seq: AtomicU64::new(0),
            cancel,
        }
    }

    /// Queue the turn on the owned runner and return the agent id its reply
    /// events will carry. The `#mw-<seq>` suffix makes every turn's id unique so
    /// a late event from a finished turn can't be read as the next turn's
    /// outcome; the runner strips the suffix back to the model slug for `--model`.
    fn dispatch(&self, worktree: &Path, task: &Task) -> Result<String, ExecutorError> {
        let turn_id = format!(
            "{}#mw-{}",
            self.model,
            self.seq.fetch_add(1, Ordering::Relaxed)
        );
        let queued = self.runner.send(ClaudeCommand::RunTurn {
            model: turn_id.clone(),
            cwd: worktree.to_path_buf(),
            mission_id: Some(self.mission_id.clone()),
            resume_session_id: None,
            persist_session: false,
            effort: None,
            prompt: task.prompt.clone(),
            read_only: false,
            max_turns: Some(INTEGRATOR_MAX_TURNS),
        });
        if queued {
            Ok(turn_id)
        } else {
            Err(ExecutorError::RunnerGone)
        }
    }

    /// Block on the private channel until this turn's terminal event, polling the
    /// cancel flag between receives so an abort during a long silent turn is still
    /// observed promptly. [`Self::classify_event`] interprets each event: `Some`
    /// ends the wait, `None` means a write was recorded (or the event was noise) so
    /// the wait continues.
    fn await_terminal(&self, turn_id: &str) -> Result<TurnResult, ExecutorError> {
        let mut changed_paths: Vec<PathBuf> = Vec::new();
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return self.cancel_run();
            }
            let event = match self.runner.events.recv_timeout(EVENT_POLL) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return Err(ExecutorError::RunnerGone),
            };
            if let Some(outcome) = self.classify_event(turn_id, event, &mut changed_paths) {
                return outcome;
            }
        }
    }

    /// Interpret one event from the owned runner. A `FileWrite` accumulates the
    /// edit signal and returns `None`, so an empty `changed_paths` at completion is
    /// a [`TurnStatus::NoOp`]. A terminal event for this turn returns `Some`; events
    /// tagged with another turn's id are late noise on the shared channel, skipped.
    fn classify_event(
        &self,
        turn_id: &str,
        event: AgentBusEvent,
        changed_paths: &mut Vec<PathBuf>,
    ) -> Option<Result<TurnResult, ExecutorError>> {
        match event {
            AgentBusEvent::FileWrite { agent_id, path, .. } if agent_id == turn_id => {
                changed_paths.push(path);
                None
            }
            AgentBusEvent::TurnCompleted {
                agent_id, message, ..
            } if agent_id == turn_id => {
                let status = if changed_paths.is_empty() {
                    TurnStatus::NoOp
                } else {
                    TurnStatus::Edited
                };
                let changed = std::mem::take(changed_paths);
                Some(Ok(TurnResult {
                    status,
                    summary: message,
                    changed_paths: changed,
                }))
            }
            AgentBusEvent::TurnFailed {
                agent_id, message, ..
            } if agent_id == turn_id => Some(self.interpret_failure(message)),
            _ => None,
        }
    }

    /// A `TurnFailed` carrying the cancel sentinel — or any failure once the
    /// operator flag is set — is an abort that propagates as `Err` to stop the
    /// search. Any other failure is a dead branch: a `Failed` result the search
    /// records and steps past, never an engine abort.
    fn interpret_failure(&self, message: String) -> Result<TurnResult, ExecutorError> {
        if message == OPERATOR_CANCEL_TURN_MESSAGE || self.cancel.load(Ordering::Relaxed) {
            return self.cancel_run();
        }
        Ok(TurnResult {
            status: TurnStatus::Failed,
            summary: message,
            changed_paths: Vec::new(),
        })
    }

    /// Stop the in-flight turn and report the cancel. The runner is private to
    /// this executor, so `CancelAll` only reaches our own turn.
    fn cancel_run(&self) -> Result<TurnResult, ExecutorError> {
        self.runner.send(ClaudeCommand::CancelAll);
        Err(ExecutorError::Cancelled)
    }
}

impl TurnExecutor for RunnerExecutor {
    type Error = ExecutorError;

    fn run_turn(&self, worktree: &Path, task: &Task) -> Result<TurnResult, Self::Error> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(ExecutorError::Cancelled);
        }
        let turn_id = self.dispatch(worktree, task)?;
        self.await_terminal(&turn_id)
    }
}

/// A cheaply-cloned handle to ONE shared [`RunnerExecutor`], so the engine's fork
/// turns and the [`JudgeMergeOracle`]'s adjudication turn drive the same runner.
///
/// The orphan rule forbids implementing the foreign [`TurnExecutor`] on a bare
/// `Rc<RunnerExecutor>` (both the trait and `Rc` are foreign here), so the share is
/// carried by this local newtype. `Rc`, not `Arc`: the engine and oracle run on one
/// search thread and `RunnerExecutor` is `!Sync` (its `ClaudeRunner` owns a `!Sync`
/// receiver), so an `Arc` would add atomic overhead for a cross-thread guarantee
/// nothing uses. The runtime builds one `SharedExecutor`, clones it into the oracle,
/// and moves the original into the engine; every clone points at the same runner,
/// event channel, and turn-id `seq` (4 fds total — the fork-width fd ceiling is
/// unchanged). Sound under serial v1: one turn is in flight at a time, so the shared
/// `seq` cannot hand out a colliding id.
#[derive(Clone)]
pub struct SharedExecutor(Rc<RunnerExecutor>);

impl SharedExecutor {
    pub fn new(executor: RunnerExecutor) -> Self {
        Self(Rc::new(executor))
    }
}

impl TurnExecutor for SharedExecutor {
    type Error = ExecutorError;

    fn run_turn(&self, worktree: &Path, task: &Task) -> Result<TurnResult, Self::Error> {
        self.0.run_turn(worktree, task)
    }
}

/// Phase 4 placeholder. The search loop never merges in v1, so adjudicating is an
/// error rather than silent wrong behaviour: a caller that reaches it has a bug.
pub struct NoMergeOracle;

impl MergeOracle for NoMergeOracle {
    type Error = ExecutorError;

    fn adjudicate(
        &self,
        _base: &Path,
        _a: &Path,
        _b: &Path,
        _conflicts: &Conflicts,
    ) -> Result<ResolvedState, Self::Error> {
        Err(ExecutorError::MergeUnsupported)
    }
}

/// The real Phase 4 merge oracle. It resolves a conflicted two-branch merge by
/// driving one write-capable judge turn whose prompt carries the three trees and
/// the conflicted paths (see [`crate::shadow::build_merge_judge_prompt`]) — the
/// inverse of the read-only shadow judge, which only ranks text.
///
/// Generic over the [`TurnExecutor`] it drives so the production oracle wraps the
/// real [`RunnerExecutor`] (a spawned `claude`) while tests script the resolution
/// through a fake — the same offline seam the engine itself is tested on, which a
/// concrete owned `ClaudeRunner` could not offer.
///
/// Frozen in-place contract: the turn runs with `cwd = a` (the "ours" worktree)
/// and edits the conflicted files there, so the resolved tree IS `a`; the engine
/// commits that worktree as the two-parent merge node.
pub struct JudgeMergeOracle<T> {
    judge: T,
}

impl<T: TurnExecutor> JudgeMergeOracle<T> {
    pub fn new(judge: T) -> Self {
        Self { judge }
    }
}

impl<T: TurnExecutor> MergeOracle for JudgeMergeOracle<T> {
    type Error = T::Error;

    fn adjudicate(
        &self,
        base: &Path,
        a: &Path,
        b: &Path,
        conflicts: &Conflicts,
    ) -> Result<ResolvedState, Self::Error> {
        let task = Task {
            prompt: crate::shadow::build_merge_judge_prompt(base, a, b, &conflicts.paths),
            role: "judge".to_owned(),
        };
        // Only an infra failure (operator cancel, runner gone) propagates and
        // aborts the search. A judge that fails to resolve leaves conflict markers
        // in `a`; the engine's valuer gates that tree, withholding a dead merge
        // branch from the frontier — never a search abort (the dead-branch rule).
        self.judge.run_turn(a, &task)?;
        Ok(ResolvedState {
            tree: a.to_path_buf(),
        })
    }
}
