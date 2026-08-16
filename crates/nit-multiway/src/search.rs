//! Best-first search loop over the world DAG.
//!
//! [`Engine::run`] seeds the search by expanding the root once, then drives a
//! frontier-ordered best-first loop: pop the highest-valued open node, stop early
//! on a top-tier solution, otherwise fork it into `k` speculative turns, value and
//! commit each viable child, and admit the strongest of them back onto the
//! frontier. `mood` decides how many siblings survive ([`frontier_admission`]);
//! the rest are recorded as `Held`. Backtracking is emergent — when a branch's
//! children regress, [`Frontier::pop_best`] simply re-selects a stronger node held
//! from an earlier expansion, so no explicit backtrack step or edge is needed. The
//! loop ends on a solution, an exhausted budget, or a drained frontier.
//!
//! [`Engine::run_merging`] adds one optional step: after an expansion values its
//! children, the two best complementary siblings may be joined into a two-parent
//! node — clean via the store, or oracle-adjudicated on conflict. A merged node
//! that fails its gates is recorded for provenance but withheld from the frontier,
//! so a dead join never aborts the search; [`Engine::run`] leaves merging off and
//! stays byte-identical for every pre-Phase-4 caller.

use std::cmp::Ordering;
use std::path::Path;

use nit_core::substrate::SubstrateState;
use serde::{Deserialize, Serialize};

use crate::edge::{Edge, EdgeKind};
use crate::frontier::Frontier;
use crate::graph::Graph;
use crate::node::{CommitRef, Node, NodeId, NodeStatus};
use crate::policy::{frontier_admission, is_solution, Budget, SearchPolicy};
use crate::traits::{
    MergeOracle, MergeOutcome, Task, TurnExecutor, TurnStatus, Valuer, WorktreeHandle, WorldStore,
};

/// Commit message for an oracle-resolved (conflicting) merge, kept distinct from
/// the store's clean-merge message so the two provenance paths stay legible in the
/// commit history.
const MERGE_MESSAGE: &str = "multiway merge (adjudicated)";

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    Solution,
    BudgetExhausted,
    FrontierEmpty,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchOutcome {
    pub best: Option<NodeId>,
    pub nodes_expanded: usize,
    pub turns_spent: usize,
    pub stop_reason: StopReason,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("world store: {0}")]
    Store(Box<dyn std::error::Error + Send + Sync>),
    #[error("turn execution: {0}")]
    Executor(Box<dyn std::error::Error + Send + Sync>),
    #[error("valuation: {0}")]
    Valuer(Box<dyn std::error::Error + Send + Sync>),
    #[error("merge adjudication: {0}")]
    Merge(Box<dyn std::error::Error + Send + Sync>),
}

/// A node is expandable iff it is `Open`, has been valued, and the value is
/// viable. FROZEN polarity (contract C5): `gated == true` means a hard gate
/// failed, so `!v.gated` is the viability test. The Phase 1 gate-exclusion test
/// asserts on this directly, without driving the whole loop.
pub fn is_expandable(node: &Node) -> bool {
    matches!(node.status, NodeStatus::Open) && node.value.as_ref().is_some_and(|v| !v.gated)
}

/// A read-only view of a run's progress, handed to the [`Observer`] after each
/// expansion. Deliberately the only window the hook gets onto [`RunStats`]: the
/// accumulator stays private so a UI closure can never reach the mutable `best`
/// it updates. Not `Copy` — `best` is a `String`-backed [`NodeId`] — but cheap to
/// clone and passed by `&`, so the hook never needs ownership.
#[derive(Clone, Debug)]
pub struct RunSnapshot {
    pub turns_spent: usize,
    pub nodes_expanded: usize,
    pub best: Option<NodeId>,
    pub best_score: f32,
}

/// Fired after every [`Engine::expand`] with the live graph, frontier, and a
/// [`RunSnapshot`]. The pure engine never depends on nit-tui: the streaming view
/// is just a `FnMut` the caller supplies, and `run`/`run_merging` pass a no-op so
/// their behavior is byte-identical to the pre-Phase-6 search.
pub type Observer<'a> = dyn FnMut(&Graph, &Frontier, &RunSnapshot) + 'a;

/// Tallies threaded through one [`Engine::run`]: the counters the outcome reports
/// plus the best viable node seen so far. `best_score` starts at negative infinity
/// so the first viable child always wins the [`f32::total_cmp`] comparison; ties
/// keep the earlier observation.
struct RunStats {
    turns_spent: usize,
    nodes_expanded: usize,
    best: Option<NodeId>,
    best_score: f32,
}

impl Default for RunStats {
    fn default() -> Self {
        Self {
            turns_spent: 0,
            nodes_expanded: 0,
            best: None,
            best_score: f32::NEG_INFINITY,
        }
    }
}

impl RunStats {
    fn observe(&mut self, id: &NodeId, score: f32) {
        if score.total_cmp(&self.best_score) == Ordering::Greater {
            self.best = Some(id.clone());
            self.best_score = score;
        }
    }

    fn snapshot(&self) -> RunSnapshot {
        RunSnapshot {
            turns_spent: self.turns_spent,
            nodes_expanded: self.nodes_expanded,
            best: self.best.clone(),
            best_score: self.best_score,
        }
    }
}

pub struct Engine<W: WorldStore, T: TurnExecutor, V: Valuer, M: MergeOracle> {
    pub store: W,
    pub executor: T,
    pub valuer: V,
    pub oracle: M,
    pub graph: Graph,
    pub frontier: Frontier,
}

impl<W, T, V, M> Engine<W, T, V, M>
where
    W: WorldStore,
    T: TurnExecutor,
    V: Valuer,
    M: MergeOracle,
{
    pub fn new(store: W, executor: T, valuer: V, oracle: M) -> Self {
        Self {
            store,
            executor,
            valuer,
            oracle,
            graph: Graph::default(),
            frontier: Frontier::default(),
        }
    }

    /// Snapshot `src` into the root node and return its id. The root is `Open` and
    /// unvalued; [`Engine::run`] expands it to seed the frontier.
    pub fn seed(&mut self, src: &Path) -> Result<NodeId, EngineError> {
        let root = self
            .store
            .snapshot(src)
            .map_err(|e| EngineError::Store(Box::new(e)))?;
        self.graph.add_node(
            root.clone(),
            Node::open(CommitRef(root.as_str().to_owned()), Vec::new()),
        );
        Ok(root)
    }

    /// Run the best-first search with merging OFF — the behavior every pre-Phase-4
    /// caller relies on. A thin wrapper over [`Engine::run_inner`] so this path stays
    /// byte-identical while [`Engine::run_merging`] opts into the Phase 4 merge step.
    pub fn run(
        &mut self,
        root: NodeId,
        policy: &SearchPolicy,
        task: &Task,
    ) -> Result<SearchOutcome, EngineError> {
        self.run_inner(root, policy, task, false, &mut |_, _, _| {})
    }

    /// Run the best-first search with the Phase 4 merge step ON: each expansion may
    /// also join its two best complementary siblings (see [`Engine::expand`]).
    pub fn run_merging(
        &mut self,
        root: NodeId,
        policy: &SearchPolicy,
        task: &Task,
    ) -> Result<SearchOutcome, EngineError> {
        self.run_inner(root, policy, task, true, &mut |_, _, _| {})
    }

    /// [`Engine::run`] with a Phase-6 streaming [`Observer`] fired after each
    /// expansion. `run` itself delegates here with a no-op, so observing changes
    /// nothing about the search — only that progress is surfaced as it lands.
    pub fn run_observed(
        &mut self,
        root: NodeId,
        policy: &SearchPolicy,
        task: &Task,
        observer: &mut Observer<'_>,
    ) -> Result<SearchOutcome, EngineError> {
        self.run_inner(root, policy, task, false, observer)
    }

    /// [`Engine::run_merging`] with the same streaming [`Observer`]; this is the
    /// production entry point, since merge is the production search path.
    pub fn run_merging_observed(
        &mut self,
        root: NodeId,
        policy: &SearchPolicy,
        task: &Task,
        observer: &mut Observer<'_>,
    ) -> Result<SearchOutcome, EngineError> {
        self.run_inner(root, policy, task, true, observer)
    }

    /// Shared driver for both entry points. Seeds the frontier by expanding `root`
    /// once outside the loop — it is unvalued, so it is never a frontier node — then
    /// pops the best open node each iteration until a solution, the budget, or an
    /// empty frontier stops it. The budget is re-checked at the top of every
    /// iteration, so seeding alone can exhaust it. `merge` threads to every expand;
    /// `observer` fires once after each expand (the seed plus every loop step), so a
    /// run's observer-call count equals its `nodes_expanded`.
    fn run_inner(
        &mut self,
        root: NodeId,
        policy: &SearchPolicy,
        task: &Task,
        merge: bool,
        observer: &mut Observer<'_>,
    ) -> Result<SearchOutcome, EngineError> {
        let mut stats = RunStats::default();
        self.expand(&root, policy, task, merge, &mut stats)?;
        observer(&self.graph, &self.frontier, &stats.snapshot());

        let stop_reason = loop {
            if self.budget_exhausted(&policy.budget, &stats) {
                break StopReason::BudgetExhausted;
            }
            let Some(id) = self.frontier.pop_best() else {
                break StopReason::FrontierEmpty;
            };
            if self.accept_if_solution(&id) {
                break StopReason::Solution;
            }
            self.expand(&id, policy, task, merge, &mut stats)?;
            observer(&self.graph, &self.frontier, &stats.snapshot());
        };

        Ok(SearchOutcome {
            best: stats.best,
            nodes_expanded: stats.nodes_expanded,
            turns_spent: stats.turns_spent,
            stop_reason,
        })
    }

    fn budget_exhausted(&self, budget: &Budget, stats: &RunStats) -> bool {
        stats.turns_spent >= budget.max_turns || self.graph.len() >= budget.max_nodes
    }

    /// Mark `id` `Solution` and report `true` when its value clears [`is_solution`];
    /// otherwise leave it for the caller to expand.
    fn accept_if_solution(&mut self, id: &NodeId) -> bool {
        let Some(node) = self.graph.node_mut(id) else {
            return false;
        };
        if node.value.is_some_and(|v| is_solution(&v)) {
            node.status = NodeStatus::Solution;
            true
        } else {
            false
        }
    }

    /// Expand `parent`: fork `k` worktrees, run+value+commit a child in each, then
    /// admit the best [`frontier_admission`] viable children to the frontier and
    /// record the rest as `Held`. NoOp/Failed turns and gate-failed children never
    /// reach the frontier; `parent` becomes `Expanded`. When `merge` is set, the two
    /// best complementary siblings are also joined via [`Engine::try_merge`] before
    /// admission — `parent` is their sole common ancestor, so the join stays acyclic.
    fn expand(
        &mut self,
        parent: &NodeId,
        policy: &SearchPolicy,
        task: &Task,
        merge: bool,
        stats: &mut RunStats,
    ) -> Result<(), EngineError> {
        stats.nodes_expanded += 1;
        let worktrees = self
            .store
            .fork(parent, policy.k)
            .map_err(|e| EngineError::Store(Box::new(e)))?;

        let mut viable: Vec<(NodeId, f32)> = Vec::new();
        for worktree in worktrees {
            stats.turns_spent += 1;
            if let Some(child) = self.commit_child(parent, worktree, task)? {
                stats.observe(&child.0, child.1);
                viable.push(child);
            }
        }

        viable.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        if merge {
            if let Some((a, b)) = crate::policy::merge_candidates(policy.mood, &viable) {
                self.try_merge(parent, &a, &b, stats)?;
            }
        }

        let admit = frontier_admission(policy.mood, viable.len());
        for (rank, (id, score)) in viable.into_iter().enumerate() {
            if rank < admit {
                self.frontier.push(id, score);
            } else if let Some(node) = self.graph.node_mut(&id) {
                node.status = NodeStatus::Held;
            }
        }

        if let Some(node) = self.graph.node_mut(parent) {
            node.status = NodeStatus::Expanded;
        }
        Ok(())
    }

    /// Run one speculative turn in `worktree`, value the resulting tree, and commit
    /// it as a child of `parent`. Returns the committed child and its score when the
    /// turn edited the tree and passed its gates. A NoOp/Failed turn commits nothing
    /// — so two identical trees can never collide on one `NodeId` — and a gate-failed
    /// child is recorded for provenance but withheld from the frontier; both yield
    /// `None`. The tree is valued live, before `commit` consumes the worktree.
    fn commit_child(
        &mut self,
        parent: &NodeId,
        worktree: W::Worktree,
        task: &Task,
    ) -> Result<Option<(NodeId, f32)>, EngineError> {
        let tree = worktree.path().to_path_buf();
        let turn = self
            .executor
            .run_turn(&tree, task)
            .map_err(|e| EngineError::Executor(Box::new(e)))?;
        if matches!(turn.status, TurnStatus::NoOp | TurnStatus::Failed) {
            return Ok(None);
        }

        let value = self
            .valuer
            .value(&tree)
            .map_err(|e| EngineError::Valuer(Box::new(e)))?;
        let child = self
            .store
            .commit(worktree, &turn.summary)
            .map_err(|e| EngineError::Store(Box::new(e)))?;

        let status = if value.gated {
            NodeStatus::GateFailed
        } else {
            NodeStatus::Open
        };
        // Capture this branch's substrate from the worktree. v1 mints none, so this
        // is the default today; it is the seam through which a v2 executor's
        // per-branch substrate will flow with no further engine change.
        let substrate = SubstrateState::load(&tree);
        let mut node = Node::child(
            CommitRef(child.as_str().to_owned()),
            value,
            status,
            parent.clone(),
            substrate,
        );
        // Phase 6 labels: thread the turn's intent onto the node post-construction
        // (the constructor signature stays frozen). `turn` is still live — `commit`
        // only borrowed its `summary`.
        node.summary = turn.summary.clone();
        node.changed_paths = turn.changed_paths.clone();
        self.graph.add_node(child.clone(), node);
        self.graph.add_edge(Edge {
            from: parent.clone(),
            to: child.clone(),
            kind: EdgeKind::Fork,
        });

        Ok((!value.gated).then_some((child, value.score)))
    }

    /// Join two complementary siblings `a` and `b` — whose sole common ancestor is
    /// `base` — into a two-parent merge node. A clean merge is committed by the store
    /// itself; a conflict is restored and handed to the oracle, which resolves into
    /// the `a` worktree before [`WorldStore::commit_merge`] records the join. The
    /// merged tree is valued like any node: one that fails its gates is recorded
    /// `GateFailed` and withheld from the frontier rather than aborting the search —
    /// only an operator cancel or a vanished runner (surfaced by the oracle)
    /// propagates as an error. A conflict spends one turn (the oracle's); a clean
    /// merge spends none, and the merged node always counts toward `max_nodes`.
    fn try_merge(
        &mut self,
        base: &NodeId,
        a: &NodeId,
        b: &NodeId,
        stats: &mut RunStats,
    ) -> Result<(), EngineError> {
        if !self.store.merge_supported() {
            return Ok(());
        }

        let (merged_id, value) = match self
            .store
            .merge(a, b)
            .map_err(|e| EngineError::Store(Box::new(e)))?
        {
            MergeOutcome::Merged(id) => {
                // git already committed and anchored the two-parent tree; restore it
                // only to value the result.
                let wt = self
                    .store
                    .restore(&id)
                    .map_err(|e| EngineError::Store(Box::new(e)))?;
                let value = self
                    .valuer
                    .value(wt.path())
                    .map_err(|e| EngineError::Valuer(Box::new(e)))?;
                (id, value)
            }
            MergeOutcome::Conflict(conflicts) => {
                let base_wt = self
                    .store
                    .restore(base)
                    .map_err(|e| EngineError::Store(Box::new(e)))?;
                let a_wt = self
                    .store
                    .restore(a)
                    .map_err(|e| EngineError::Store(Box::new(e)))?;
                let b_wt = self
                    .store
                    .restore(b)
                    .map_err(|e| EngineError::Store(Box::new(e)))?;
                stats.turns_spent += 1;
                self.oracle
                    .adjudicate(base_wt.path(), a_wt.path(), b_wt.path(), &conflicts)
                    .map_err(|e| EngineError::Merge(Box::new(e)))?;
                // Value the resolved `a` worktree before `commit_merge` consumes it.
                let value = self
                    .valuer
                    .value(a_wt.path())
                    .map_err(|e| EngineError::Valuer(Box::new(e)))?;
                let merged = self
                    .store
                    .commit_merge(a_wt, a, b, MERGE_MESSAGE)
                    .map_err(|e| EngineError::Store(Box::new(e)))?;
                (merged, value)
            }
        };

        let sa = self
            .graph
            .node(a)
            .map(|n| n.substrate.clone())
            .unwrap_or_default();
        let sb = self
            .graph
            .node(b)
            .map(|n| n.substrate.clone())
            .unwrap_or_default();
        let status = if value.gated {
            NodeStatus::GateFailed
        } else {
            NodeStatus::Open
        };
        let mut node = Node::merged(
            CommitRef(merged_id.as_str().to_owned()),
            value,
            status,
            [a.clone(), b.clone()],
            SubstrateState::reconcile(&sa, &sb),
        );
        // A join carries no per-file delta of its own; label it as the merge it is.
        node.summary = "merge".to_owned();
        self.graph.add_node(merged_id.clone(), node);
        self.graph.add_edge(Edge {
            from: a.clone(),
            to: merged_id.clone(),
            kind: EdgeKind::Merge,
        });
        self.graph.add_edge(Edge {
            from: b.clone(),
            to: merged_id.clone(),
            kind: EdgeKind::Merge,
        });

        stats.observe(&merged_id, value.score);
        if !value.gated {
            self.frontier.push(merged_id, value.score);
        }
        Ok(())
    }
}
