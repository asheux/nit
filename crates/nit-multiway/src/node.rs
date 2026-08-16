use std::fmt;
use std::path::PathBuf;

use nit_core::substrate::SubstrateState;
use serde::{Deserialize, Serialize};

use crate::value::Value;

/// Engine identity and graph key. `GitWorldStore` derives it from the commit
/// SHA; the fake store uses the blake3 hex of the scripted world content.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(String);

impl NodeId {
    pub fn new(addr: impl Into<String>) -> Self {
        Self(addr.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The store-specific VCS handle a node was committed as. For `GitWorldStore`
/// it coincides in value with the `NodeId` (both the SHA) but differs in role;
/// for the fake store the `NodeId` is a blake3 of content while this is an
/// in-memory tree key, so the two genuinely diverge — which is what earns the
/// `commit` field its place on [`Node`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitRef(pub String);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    /// Viable and eligible for the frontier.
    Open,
    /// Already forked into children.
    Expanded,
    /// Set after observing `value.gated == true`: non-viable, excluded from
    /// expansion, but retained in the DAG for provenance.
    GateFailed,
    /// Phase 2: pruned because a sibling strictly dominates it.
    Dominated,
    /// Phase 2: set aside as a backtrack candidate.
    Held,
    /// Accepted terminal state.
    Solution,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub commit: CommitRef,
    /// `None` until the valuer has scored the node.
    pub value: Option<Value>,
    pub status: NodeStatus,
    /// 0 parents = root, 1 = Turn/Fork/Backtrack, 2 = Merge.
    pub parents: Vec<NodeId>,
    /// Per-branch substrate captured from the node's worktree; a merge
    /// reconciles the two parents' snapshots via [`SubstrateState::reconcile`].
    /// `#[serde(default)]` keeps pre-Phase-4 persisted DAGs loadable. Default in
    /// v1 — no executor mints per-branch substrate yet (`docs/MULTIWAY.md`).
    #[serde(default)]
    pub substrate: SubstrateState,
    /// What the agent's turn attempted, copied from `TurnResult::summary` so the
    /// Phase-6 graph view can label a branch with its intent, not just a SHA.
    /// Threaded *after* construction in [`crate::search`] (the `child`/`merged`
    /// constructors stay frozen so `tests/` call sites don't churn);
    /// `#[serde(default)]` keeps pre-Phase-6 DAGs — which carry no label key —
    /// loadable, mirroring `substrate`.
    #[serde(default)]
    pub summary: String,
    /// Paths the turn touched (`TurnResult::changed_paths`), rendered as a file
    /// count in the view. Empty for the root and a clean merge, which mint no
    /// per-file delta of their own.
    #[serde(default)]
    pub changed_paths: Vec<PathBuf>,
}

impl Node {
    /// A root or any pre-valuation node: unvalued, `Open`, default substrate.
    pub fn open(commit: CommitRef, parents: Vec<NodeId>) -> Self {
        Self {
            commit,
            value: None,
            status: NodeStatus::Open,
            parents,
            substrate: SubstrateState::default(),
            summary: String::new(),
            changed_paths: Vec::new(),
        }
    }

    /// A single-parent Turn/Fork child carrying its captured worktree substrate.
    pub fn child(
        commit: CommitRef,
        value: Value,
        status: NodeStatus,
        parent: NodeId,
        substrate: SubstrateState,
    ) -> Self {
        Self {
            commit,
            value: Some(value),
            status,
            parents: vec![parent],
            substrate,
            summary: String::new(),
            changed_paths: Vec::new(),
        }
    }

    /// A two-parent merged node carrying the reconciled substrate.
    pub fn merged(
        commit: CommitRef,
        value: Value,
        status: NodeStatus,
        parents: [NodeId; 2],
        substrate: SubstrateState,
    ) -> Self {
        Self {
            commit,
            value: Some(value),
            status,
            parents: parents.to_vec(),
            substrate,
            summary: String::new(),
            changed_paths: Vec::new(),
        }
    }
}
