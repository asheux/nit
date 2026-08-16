//! Engine seams: the four traits the search loop runs against, so Phases 0-2 are
//! testable with the in-memory fakes in [`crate::testing`] instead of real git
//! and spawned agents. `Task`, `TurnResult`, `Conflicts`, `MergeOutcome` and
//! `ResolvedState` are concrete shared vocabulary — associating them would force
//! every `Engine` bound to name them and defeat the seam.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::node::NodeId;
use crate::value::Value;

pub trait WorktreeHandle {
    fn path(&self) -> &Path;
    fn parent(&self) -> &NodeId;
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Conflicts {
    pub paths: Vec<PathBuf>,
}

/// The three results of a merge: a clean join (`Merged`), a semantic conflict
/// that is a normal search event for the Phase 4 oracle (`Conflict`), and — kept
/// out of this enum — an infra failure (git spawn, git < 2.38) surfaced as the
/// store's `Error`. A two-case `Result<NodeId, Conflicts>` has nowhere to put
/// the third without panicking or swallowing it.
#[derive(Clone, Debug)]
pub enum MergeOutcome {
    Merged(NodeId),
    Conflict(Conflicts),
}

pub trait WorldStore {
    type Worktree: WorktreeHandle;
    type Error: std::error::Error + Send + Sync + 'static;

    fn snapshot(&self, src: &Path) -> Result<NodeId, Self::Error>;
    fn fork(&self, parent: &NodeId, k: usize) -> Result<Vec<Self::Worktree>, Self::Error>;
    fn commit(&self, worktree: Self::Worktree, message: &str) -> Result<NodeId, Self::Error>;
    fn merge(&self, a: &NodeId, b: &NodeId) -> Result<MergeOutcome, Self::Error>;
    /// Commit an oracle-resolved worktree as a two-parent merge node. Distinct
    /// from `commit` because a join must record *both* parents explicitly — the
    /// single `worktree.parent()` a fork carries cannot express a merge.
    fn commit_merge(
        &self,
        worktree: Self::Worktree,
        a: &NodeId,
        b: &NodeId,
        message: &str,
    ) -> Result<NodeId, Self::Error>;
    /// Whether `merge` can run on this backend (object-level `merge-tree` needs
    /// git >= 2.38). The engine queries this *before* calling `merge`, so an
    /// unsupported backend degrades to fork-only search instead of aborting.
    fn merge_supported(&self) -> bool;
    fn restore(&self, node: &NodeId) -> Result<Self::Worktree, Self::Error>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub prompt: String,
    pub role: String,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnStatus {
    Edited,
    NoOp,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TurnResult {
    pub status: TurnStatus,
    pub summary: String,
    pub changed_paths: Vec<PathBuf>,
}

pub trait TurnExecutor {
    type Error: std::error::Error + Send + Sync + 'static;
    /// Mutates files under `worktree` in place and reports what changed.
    fn run_turn(&self, worktree: &Path, task: &Task) -> Result<TurnResult, Self::Error>;
}

pub trait Valuer {
    type Error: std::error::Error + Send + Sync + 'static;
    /// Values a tree by path, not by `NodeId`: a `Value` is a pure function of
    /// tree contents, so the engine values the live worktree before `commit`
    /// consumes it — no extra checkout, and `commit` stays by-value.
    fn value(&self, tree: &Path) -> Result<Value, Self::Error>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolvedState {
    pub tree: PathBuf,
}

pub trait MergeOracle {
    type Error: std::error::Error + Send + Sync + 'static;
    fn adjudicate(
        &self,
        base: &Path,
        a: &Path,
        b: &Path,
        conflicts: &Conflicts,
    ) -> Result<ResolvedState, Self::Error>;
}
