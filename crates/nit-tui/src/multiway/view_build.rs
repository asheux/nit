//! Projects the live `nit-multiway` search state into the render-only
//! [`MultiwayView`] the Phase 6b popup draws. The pure engine fires its observer
//! with `(&Graph, &Frontier, &RunSnapshot)` after every expansion; nit-core can't
//! name those engine types (the crate edge runs nit-multiway → nit-core), so the
//! projection lives here, where both sides are in scope. It is called once per
//! expansion, so it stays cheap: one pre-order walk of the DAG plus a lineage
//! trace, no I/O and no allocation beyond the view itself.

use nit_core::{GenomeTier, MultiwayHeader, MultiwayNodeStatus, MultiwayNodeView, MultiwayView};
use nit_multiway::frontier::Frontier;
use nit_multiway::graph::Graph;
use nit_multiway::node::{Node, NodeId, NodeStatus};
use nit_multiway::policy::{Mood, SearchPolicy};
use nit_multiway::search::{RunSnapshot, StopReason};

/// Commit-id prefix kept for a row label — enough to disambiguate within one small
/// search DAG without crowding a terminal row.
const SHORT_ID_LEN: usize = 8;

pub fn build_multiway_view(
    graph: &Graph,
    frontier: &Frontier,
    snap: &RunSnapshot,
    policy: &SearchPolicy,
    stop_reason: Option<StopReason>,
) -> MultiwayView {
    let nodes = ordered_with_depth(graph)
        .into_iter()
        .filter_map(|(id, depth)| graph.node(&id).map(|node| node_view(&id, depth, node)))
        .collect();
    MultiwayView {
        header: build_header(graph, frontier, snap, policy, stop_reason),
        nodes,
        kept_path: best_lineage(graph, snap.best.as_ref()),
        in_flight: frontier.peek_best().map(|id| short_id(id.as_str())),
    }
}

fn build_header(
    graph: &Graph,
    frontier: &Frontier,
    snap: &RunSnapshot,
    policy: &SearchPolicy,
    stop_reason: Option<StopReason>,
) -> MultiwayHeader {
    // `RunStats` seeds `best_score` at -inf, so read it only once a best exists;
    // the tier comes off that node's value.
    let (best_score, best_tier) = match snap.best.as_ref() {
        Some(id) => (
            snap.best_score,
            graph
                .node(id)
                .and_then(|node| node.value.as_ref())
                .map(|value| value.tier)
                .unwrap_or(GenomeTier::StillLife),
        ),
        None => (0.0, GenomeTier::StillLife),
    };
    MultiwayHeader {
        mood: mood_label(policy.mood).to_owned(),
        k: policy.k,
        turns: snap.turns_spent,
        max_turns: policy.budget.max_turns,
        nodes: graph.len(),
        max_nodes: policy.budget.max_nodes,
        frontier: frontier.len(),
        best_score,
        best_tier,
        stop_reason: stop_reason.map(|reason| stop_label(reason).to_owned()),
    }
}

fn node_view(id: &NodeId, depth: u16, node: &Node) -> MultiwayNodeView {
    MultiwayNodeView {
        id: id.as_str().to_owned(),
        short_id: short_id(id.as_str()),
        depth,
        status: project_status(node.status),
        score: node.value.map(|value| value.score),
        tier: node.value.map(|value| value.tier),
        summary: node.summary.clone(),
        changed: node.changed_paths.len(),
        parents: node.parents.iter().map(|p| short_id(p.as_str())).collect(),
    }
}

/// Pre-order DAG walk (parent before children) paired with each node's tree depth,
/// so the popup can indent rows without re-deriving lineage. Children are visited
/// in id order for a stable layout; a merge node is emitted once, at the depth of
/// whichever parent reaches it first. The `seen` set both dedupes a diamond and
/// guards against an accidental cycle, and any node not reachable from a root
/// (never expected) is appended so the view can never silently drop one.
fn ordered_with_depth(graph: &Graph) -> Vec<(NodeId, u16)> {
    let mut roots: Vec<NodeId> = graph
        .iter_nodes()
        .filter(|(_, node)| node.parents.is_empty())
        .map(|(id, _)| id.clone())
        .collect();
    roots.sort();

    let mut order = Vec::with_capacity(graph.len());
    let mut seen = std::collections::BTreeSet::new();
    let mut stack: Vec<(NodeId, u16)> = roots.into_iter().rev().map(|id| (id, 0)).collect();
    while let Some((id, depth)) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        order.push((id.clone(), depth));
        let mut children = graph.children(&id);
        children.sort();
        for child in children.into_iter().rev() {
            if !seen.contains(&child) {
                stack.push((child, depth.saturating_add(1)));
            }
        }
    }
    for (id, _) in graph.iter_nodes() {
        if seen.insert(id.clone()) {
            order.push((id.clone(), 0));
        }
    }
    order
}

/// First-parent lineage from the best node up to a root, returned root-first so the
/// popup can highlight the kept path top-down. The `guard` bounds the walk by the
/// node count in case a malformed DAG ever loops.
fn best_lineage(graph: &Graph, best: Option<&NodeId>) -> Vec<String> {
    let Some(best) = best else {
        return Vec::new();
    };
    let mut lineage = Vec::new();
    let mut current = best.clone();
    let mut guard = 0;
    loop {
        lineage.push(short_id(current.as_str()));
        match graph.parents(&current).first() {
            Some(parent) if guard < graph.len() => {
                current = parent.clone();
                guard += 1;
            }
            _ => break,
        }
    }
    lineage.reverse();
    lineage
}

fn short_id(id: &str) -> String {
    id.chars().take(SHORT_ID_LEN).collect()
}

fn project_status(status: NodeStatus) -> MultiwayNodeStatus {
    match status {
        NodeStatus::Open => MultiwayNodeStatus::Open,
        NodeStatus::Expanded => MultiwayNodeStatus::Expanded,
        NodeStatus::GateFailed => MultiwayNodeStatus::GateFailed,
        NodeStatus::Dominated => MultiwayNodeStatus::Dominated,
        NodeStatus::Held => MultiwayNodeStatus::Held,
        NodeStatus::Solution => MultiwayNodeStatus::Solution,
    }
}

fn mood_label(mood: Mood) -> &'static str {
    match mood {
        Mood::Explore => "explore",
        Mood::Balanced => "balanced",
        Mood::Exploit => "exploit",
    }
}

fn stop_label(reason: StopReason) -> &'static str {
    match reason {
        StopReason::Solution => "solution",
        StopReason::BudgetExhausted => "budget",
        StopReason::FrontierEmpty => "frontier-empty",
    }
}
