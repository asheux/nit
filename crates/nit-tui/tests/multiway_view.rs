//! Locks `build_multiway_view`, the projection the Phase 6b popup renders from a
//! live search. The engine fires its observer with `(&Graph, &Frontier,
//! &RunSnapshot)` after every expansion, and this view is the only window the popup
//! gets — a silent drift in the mapping would surface as a wrong tree with no other
//! signal. Over a fixture DAG (a root forked into a kept solution, a gate-failed
//! dead end, a held sibling, and an open frontier node) the tests pin each node's
//! status/value/label, the root→solution kept path, the frontier's next pick, and
//! the header readout, plus the empty / root-only / merge edge cases.

use std::path::PathBuf;

use nit_core::{GenomeTier, MultiwayNodeStatus, MultiwayNodeView, MultiwayView, SubstrateState};
use nit_multiway::edge::{Edge, EdgeKind};
use nit_multiway::frontier::Frontier;
use nit_multiway::graph::Graph;
use nit_multiway::node::{CommitRef, Node, NodeId, NodeStatus};
use nit_multiway::policy::{Budget, Mood, SearchPolicy};
use nit_multiway::search::{RunSnapshot, StopReason};
use nit_multiway::Value;

use nit_tui::multiway::build_multiway_view;

// Twelve-char ids so the view's eight-char truncation is exercised, not bypassed.
const ROOT: &str = "root00000000";
const WIN: &str = "win111111111";
const DEAD: &str = "dead22222222";
const HELD: &str = "held33333333";
const OPEN: &str = "open44444444";

/// The row label the view derives from an id; mirrors the projection's width so the
/// kept-path, parent, and next-pick assertions can name nodes by their short id.
fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

fn node<'v>(view: &'v MultiwayView, id: &str) -> &'v MultiwayNodeView {
    view.nodes
        .iter()
        .find(|n| n.id == id)
        .unwrap_or_else(|| panic!("{id} absent from the view"))
}

fn policy() -> SearchPolicy {
    SearchPolicy {
        mood: Mood::Explore,
        k: 3,
        budget: Budget {
            max_nodes: 64,
            max_turns: 32,
            max_tokens: None,
        },
    }
}

/// A root expanded into one child per search status the popup distinguishes. `win`
/// also carries a summary and two changed files so the label fields travel through
/// the projection, not just the status.
fn forked_dag() -> Graph {
    let mut graph = Graph::default();
    let root = NodeId::new(ROOT);

    let mut root_node = Node::open(CommitRef("c0".into()), vec![]);
    root_node.status = NodeStatus::Expanded;
    graph.add_node(root.clone(), root_node);

    let mut win = Node::child(
        CommitRef("c1".into()),
        Value {
            gated: false,
            score: 0.91,
            tier: GenomeTier::Replicator,
        },
        NodeStatus::Solution,
        root.clone(),
        SubstrateState::default(),
    );
    win.summary = "land the solution".to_owned();
    win.changed_paths = vec![PathBuf::from("core.rs"), PathBuf::from("view.rs")];
    graph.add_node(NodeId::new(WIN), win);

    graph.add_node(
        NodeId::new(DEAD),
        Node::child(
            CommitRef("c2".into()),
            Value {
                gated: true,
                score: 0.10,
                tier: GenomeTier::StillLife,
            },
            NodeStatus::GateFailed,
            root.clone(),
            SubstrateState::default(),
        ),
    );
    graph.add_node(
        NodeId::new(HELD),
        Node::child(
            CommitRef("c3".into()),
            Value {
                gated: false,
                score: 0.40,
                tier: GenomeTier::Spaceship,
            },
            NodeStatus::Held,
            root.clone(),
            SubstrateState::default(),
        ),
    );
    graph.add_node(
        NodeId::new(OPEN),
        Node::child(
            CommitRef("c4".into()),
            Value {
                gated: false,
                score: 0.55,
                tier: GenomeTier::Methuselah,
            },
            NodeStatus::Open,
            root.clone(),
            SubstrateState::default(),
        ),
    );

    for (to, kind) in [
        (WIN, EdgeKind::Turn),
        (DEAD, EdgeKind::Fork),
        (HELD, EdgeKind::Fork),
        (OPEN, EdgeKind::Fork),
    ] {
        graph.add_edge(Edge {
            from: root.clone(),
            to: NodeId::new(to),
            kind,
        });
    }
    graph
}

/// The fixture mid-search: the DAG, a frontier holding only the open node, and a
/// snapshot whose best is the solution.
fn expanded_search() -> (Graph, Frontier, RunSnapshot) {
    let mut frontier = Frontier::default();
    frontier.push(NodeId::new(OPEN), 0.55);
    let snap = RunSnapshot {
        turns_spent: 4,
        nodes_expanded: 1,
        best: Some(NodeId::new(WIN)),
        best_score: 0.91,
    };
    (forked_dag(), frontier, snap)
}

#[test]
fn fixture_dag_maps_statuses_kept_path_and_next_pick() {
    let (graph, frontier, snap) = expanded_search();
    let view = build_multiway_view(
        &graph,
        &frontier,
        &snap,
        &policy(),
        Some(StopReason::Solution),
    );

    assert_eq!(view.nodes.len(), 5, "every node is projected once");

    let root = node(&view, ROOT);
    assert_eq!(root.status, MultiwayNodeStatus::Expanded);
    assert_eq!(root.depth, 0);
    assert!(
        root.score.is_none(),
        "an unvalued root has no score or tier"
    );
    assert!(root.tier.is_none());
    assert!(root.parents.is_empty());

    let win = node(&view, WIN);
    assert_eq!(win.id, WIN, "the full id is preserved");
    assert_eq!(win.short_id, short(WIN), "and truncated to the row label");
    assert_eq!(win.status, MultiwayNodeStatus::Solution);
    assert_eq!(win.depth, 1);
    assert_eq!(win.score, Some(0.91));
    assert_eq!(win.tier, Some(GenomeTier::Replicator));
    assert_eq!(win.summary, "land the solution");
    assert_eq!(win.changed, 2, "changed paths surface as a count");
    assert_eq!(win.parents, vec![short(ROOT)]);

    for (id, status) in [
        (DEAD, MultiwayNodeStatus::GateFailed),
        (HELD, MultiwayNodeStatus::Held),
        (OPEN, MultiwayNodeStatus::Open),
    ] {
        assert_eq!(node(&view, id).status, status, "status of {id}");
    }

    // The kept path is root-first and ends at the solution the snapshot named best.
    assert_eq!(view.kept_path, vec![short(ROOT), short(WIN)]);

    // The marked next pick is exactly the frontier's best.
    assert_eq!(
        view.in_flight,
        frontier.peek_best().map(|id| short(id.as_str()))
    );
    assert_eq!(view.in_flight, Some(short(OPEN)));
}

#[test]
fn header_carries_policy_budget_and_stop_reason() {
    let (graph, frontier, snap) = expanded_search();
    let header = build_multiway_view(
        &graph,
        &frontier,
        &snap,
        &policy(),
        Some(StopReason::BudgetExhausted),
    )
    .header;

    assert_eq!(header.mood, "explore");
    assert_eq!(header.k, 3);
    assert_eq!(header.turns, 4);
    assert_eq!(header.max_turns, 32);
    assert_eq!(header.nodes, 5);
    assert_eq!(header.max_nodes, 64);
    assert_eq!(header.frontier, 1);
    assert_eq!(header.best_score, 0.91);
    assert_eq!(header.best_tier, GenomeTier::Replicator);
    assert_eq!(header.stop_reason.as_deref(), Some("budget"));
}

#[test]
fn empty_and_root_only_graphs_never_panic() {
    let empty = build_multiway_view(
        &Graph::default(),
        &Frontier::default(),
        &RunSnapshot {
            turns_spent: 0,
            nodes_expanded: 0,
            best: None,
            best_score: 0.0,
        },
        &policy(),
        None,
    );
    assert!(empty.nodes.is_empty());
    assert!(empty.kept_path.is_empty());
    assert!(empty.in_flight.is_none());
    assert_eq!(empty.header.nodes, 0);
    assert_eq!(empty.header.best_tier, GenomeTier::StillLife);
    assert!(empty.header.stop_reason.is_none());

    let mut graph = Graph::default();
    let root = NodeId::new("solo00000000");
    graph.add_node(root.clone(), Node::open(CommitRef("c0".into()), vec![]));
    let snap = RunSnapshot {
        turns_spent: 0,
        nodes_expanded: 1,
        best: Some(root.clone()),
        best_score: 0.0,
    };

    let view = build_multiway_view(&graph, &Frontier::default(), &snap, &policy(), None);
    assert_eq!(view.nodes.len(), 1);
    assert_eq!(view.nodes[0].depth, 0);
    assert_eq!(view.nodes[0].status, MultiwayNodeStatus::Open);
    assert_eq!(view.kept_path, vec![short("solo00000000")]);
    assert!(
        view.in_flight.is_none(),
        "an empty frontier marks no next pick"
    );
}

#[test]
fn merge_node_projects_both_parents() {
    let mut graph = Graph::default();
    let left = NodeId::new("leftaaaa0000");
    let right = NodeId::new("rightbbbb000");
    let merged = NodeId::new("mergecccc000");

    graph.add_node(left.clone(), Node::open(CommitRef("l".into()), vec![]));
    graph.add_node(right.clone(), Node::open(CommitRef("r".into()), vec![]));
    graph.add_node(
        merged.clone(),
        Node::merged(
            CommitRef("m".into()),
            Value {
                gated: false,
                score: 0.70,
                tier: GenomeTier::Methuselah,
            },
            NodeStatus::Open,
            [left.clone(), right.clone()],
            SubstrateState::default(),
        ),
    );
    graph.add_edge(Edge {
        from: left.clone(),
        to: merged.clone(),
        kind: EdgeKind::Merge,
    });
    graph.add_edge(Edge {
        from: right.clone(),
        to: merged.clone(),
        kind: EdgeKind::Merge,
    });

    let snap = RunSnapshot {
        turns_spent: 2,
        nodes_expanded: 1,
        best: Some(merged.clone()),
        best_score: 0.70,
    };
    let view = build_multiway_view(&graph, &Frontier::default(), &snap, &policy(), None);

    assert_eq!(view.nodes.len(), 3);
    let merged_view = node(&view, "mergecccc000");
    assert_eq!(
        merged_view.parents,
        vec![short("leftaaaa0000"), short("rightbbbb000")],
        "a Phase-4 merge keeps both parents, in order"
    );

    // First-parent lineage settles the kept path on the merged best via parents[0].
    assert_eq!(
        view.kept_path,
        vec![short("leftaaaa0000"), short("mergecccc000")]
    );
}
