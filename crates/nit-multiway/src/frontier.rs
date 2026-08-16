//! Best-first frontier: the ordered container of nodes eligible for expansion.
//! Phase 0/1 provides only the container and its ordering; the mood-driven
//! backtracking that re-selects held nodes is Phase 2 (extension point below).

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::graph::Graph;
use crate::node::{NodeId, NodeStatus};

/// Ordered view of the nodes currently eligible for expansion, ranked by node
/// value score (higher is better).
///
/// Not serialized: a search rebuilds it from the persisted [`Graph`] on load via
/// [`Frontier::rebuild_from`]. Ordering uses [`f32::total_cmp`] so a stray NaN can
/// never panic the heap, and equal scores break on [`NodeId`] so the same graph
/// always expands in the same order.
#[derive(Clone, Debug, Default)]
pub struct Frontier {
    heap: BinaryHeap<Ranked>,
}

impl Frontier {
    pub fn push(&mut self, id: NodeId, score: f32) {
        self.heap.push(Ranked { score, id });
    }

    pub fn pop_best(&mut self) -> Option<NodeId> {
        self.heap.pop().map(|ranked| ranked.id)
    }

    pub fn peek_best(&self) -> Option<&NodeId> {
        self.heap.peek().map(|ranked| &ranked.id)
    }

    /// Replace the ordering with every Open, valued, viable node in `graph`.
    /// Gate-failed and not-yet-valued nodes are deliberately omitted — only an
    /// expandable node belongs on the frontier.
    pub fn rebuild_from(&mut self, graph: &Graph) {
        self.heap.clear();
        for (id, node) in graph.iter_nodes() {
            let Some(value) = &node.value else { continue };
            if node.status == NodeStatus::Open && value.is_viable() {
                self.push(id.clone(), value.score);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}

// Phase 2 extension point: best-first backtracking layers on top of this container.
// When a branch's children regress, the policy re-selects a held node instead of
// the global max. That logic belongs in `policy.rs`/`search.rs` and only changes
// which id is drawn next, never the heap's contents — do not anticipate it here.

/// A node paired with its score for heap ordering. `Ord` is score-major via
/// [`f32::total_cmp`]; equal scores fall back to the [`NodeId`], reversed so the
/// lexicographically smaller id is drawn first and ties stay deterministic.
#[derive(Clone, Debug)]
struct Ranked {
    score: f32,
    id: NodeId,
}

impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| other.id.cmp(&self.id))
    }
}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Ranked {}
