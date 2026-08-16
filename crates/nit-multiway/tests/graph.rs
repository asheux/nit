use std::path::PathBuf;

use nit_core::{GenomeTier, SubstrateState};
use nit_multiway::edge::{Edge, EdgeKind};
use nit_multiway::graph::Graph;
use nit_multiway::node::{CommitRef, Node, NodeId, NodeStatus};
use nit_multiway::Value;

fn leaf(commit: &str, status: NodeStatus, parents: Vec<NodeId>) -> Node {
    Node {
        commit: CommitRef(commit.to_string()),
        value: None,
        status,
        parents,
        substrate: SubstrateState::default(),
        summary: String::new(),
        changed_paths: Vec::new(),
    }
}

/// A diamond DAG: `root` forks into `left`/`right`, which merge into `merged`.
/// Covers a multi-parent node plus the Turn/Fork/Merge edge kinds.
fn diamond() -> Graph {
    let mut graph = Graph::default();
    let root = NodeId::new("root");
    let left = NodeId::new("left");
    let right = NodeId::new("right");
    let merged = NodeId::new("merged");

    graph.add_node(root.clone(), leaf("c0", NodeStatus::Expanded, vec![]));
    graph.add_node(
        left.clone(),
        leaf("c1", NodeStatus::Open, vec![root.clone()]),
    );
    graph.add_node(
        right.clone(),
        leaf("c2", NodeStatus::GateFailed, vec![root.clone()]),
    );
    graph.add_node(
        merged.clone(),
        leaf(
            "c3",
            NodeStatus::Solution,
            vec![left.clone(), right.clone()],
        ),
    );

    graph.add_edge(Edge {
        from: root.clone(),
        to: left.clone(),
        kind: EdgeKind::Turn,
    });
    graph.add_edge(Edge {
        from: root,
        to: right.clone(),
        kind: EdgeKind::Fork,
    });
    graph.add_edge(Edge {
        from: left,
        to: merged.clone(),
        kind: EdgeKind::Merge,
    });
    graph.add_edge(Edge {
        from: right,
        to: merged,
        kind: EdgeKind::Merge,
    });
    graph
}

#[test]
fn children_follow_edges_and_parents_follow_nodes() {
    let graph = diamond();
    assert_eq!(graph.len(), 4);
    assert!(!graph.is_empty());

    assert_eq!(
        graph.children(&NodeId::new("root")),
        vec![NodeId::new("left"), NodeId::new("right")]
    );
    assert_eq!(
        graph.parents(&NodeId::new("merged")).to_vec(),
        vec![NodeId::new("left"), NodeId::new("right")]
    );
    assert!(graph.children(&NodeId::new("merged")).is_empty());
    assert!(graph.parents(&NodeId::new("root")).is_empty());
}

#[test]
fn dag_round_trips_through_serde_json() {
    let graph = diamond();
    let json = serde_json::to_string(&graph).expect("serialize");
    let restored: Graph = serde_json::from_str(&json).expect("deserialize");

    // Lossless round-trip: re-encoding the restored graph reproduces the original
    // bytes. BTreeMap node keys and the ordered edge vec keep encoding deterministic.
    assert_eq!(
        json,
        serde_json::to_string(&restored).expect("re-serialize")
    );

    assert_eq!(restored.len(), graph.len());
    assert_eq!(
        restored.children(&NodeId::new("root")),
        vec![NodeId::new("left"), NodeId::new("right")]
    );
    assert_eq!(
        restored.parents(&NodeId::new("merged")).to_vec(),
        vec![NodeId::new("left"), NodeId::new("right")]
    );
    assert_eq!(
        restored
            .node(&NodeId::new("merged"))
            .expect("merged node present")
            .status,
        NodeStatus::Solution
    );
}

#[test]
fn save_then_load_preserves_node_values_through_a_file() {
    let mut dag = Graph::default();
    let seed = NodeId::new("seed");
    let improved = NodeId::new("improved");
    dag.add_node(
        seed.clone(),
        Node {
            commit: CommitRef("c0".into()),
            value: Some(Value {
                gated: false,
                score: 0.42,
                tier: GenomeTier::Spaceship,
            }),
            status: NodeStatus::Expanded,
            parents: vec![],
            substrate: SubstrateState::default(),
            summary: String::new(),
            changed_paths: Vec::new(),
        },
    );
    dag.add_node(
        improved.clone(),
        Node {
            commit: CommitRef("c1".into()),
            value: Some(Value {
                gated: false,
                score: 0.87,
                tier: GenomeTier::Replicator,
            }),
            status: NodeStatus::Open,
            parents: vec![seed.clone()],
            substrate: SubstrateState::default(),
            summary: String::new(),
            changed_paths: Vec::new(),
        },
    );
    dag.add_edge(Edge {
        from: seed,
        to: improved.clone(),
        kind: EdgeKind::Fork,
    });

    // Writing through a missing intermediate directory proves `save` creates it;
    // cleanup at both ends keeps a panicking run from poisoning the next.
    let scratch = std::env::temp_dir().join(format!("nit_mw_graph_save_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let dag_path = scratch.join("nested").join("dag.json");

    dag.save(&dag_path)
        .expect("save creates parents and writes atomically");
    let restored = Graph::load(&dag_path).expect("load reads it back");

    // Byte-identical re-encoding proves the f32 scores survived the round-trip —
    // the gap the diamond round-trip (every value `None`) never exercises.
    assert_eq!(
        serde_json::to_string(&dag).expect("encode original"),
        serde_json::to_string(&restored).expect("encode restored"),
    );
    let child = restored.node(&improved).expect("child present after load");
    assert_eq!(
        child.value.expect("child kept its value").tier,
        GenomeTier::Replicator
    );
    assert_eq!(restored.len(), 2);

    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn to_dot_renders_the_diamond_with_typed_edges() {
    let dot = diamond().to_dot();

    // A well-formed document: the header, a closing brace, and every brace balanced.
    assert!(dot.starts_with("digraph multiway {"));
    assert!(dot.trim_end().ends_with('}'));
    assert_eq!(
        dot.matches('{').count(),
        dot.matches('}').count(),
        "unbalanced braces:\n{dot}"
    );

    // Every node is emitted as a quoted statement and every parent→child edge as a
    // typed arrow — the diamond carries one of each edge kind it was built with.
    for id in ["root", "left", "right", "merged"] {
        assert!(dot.contains(&format!("\"{id}\"")), "missing node {id}");
    }
    assert!(dot.contains("\"root\" -> \"left\" [label=\"Turn\"]"));
    assert!(dot.contains("\"root\" -> \"right\" [label=\"Fork\"]"));
    assert!(dot.contains("label=\"Merge\""));

    // One node statement per node and one arrow per edge: the diamond's four nodes
    // (each carrying a `shape=`) and four edges, nothing duplicated or dropped.
    assert_eq!(
        dot.matches("shape=").count(),
        4,
        "one statement per node:\n{dot}"
    );
    assert_eq!(dot.matches(" -> ").count(), 4, "one arrow per edge:\n{dot}");

    // Status drives shape: the accepted solution is a doublecircle, the gate-failed
    // sibling an octagon.
    assert!(dot.contains("shape=doublecircle"));
    assert!(dot.contains("shape=octagon"));
}

#[test]
fn to_dot_labels_values_and_marks_the_kept_path() {
    let mut graph = Graph::default();
    let root = NodeId::new("rootsha0");
    let win = NodeId::new("winsha00");
    graph.add_node(
        root.clone(),
        Node {
            commit: CommitRef("c0".into()),
            value: Some(Value {
                gated: false,
                score: 0.40,
                tier: GenomeTier::Spaceship,
            }),
            status: NodeStatus::Expanded,
            parents: vec![],
            substrate: SubstrateState::default(),
            summary: "seed the module".into(),
            changed_paths: vec![PathBuf::from("src/lib.rs")],
        },
    );
    graph.add_node(
        win.clone(),
        Node {
            commit: CommitRef("c1".into()),
            value: Some(Value {
                gated: false,
                score: 0.95,
                tier: GenomeTier::Replicator,
            }),
            status: NodeStatus::Solution,
            parents: vec![root.clone()],
            substrate: SubstrateState::default(),
            summary: "land the fix".into(),
            changed_paths: vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")],
        },
    );
    graph.add_edge(Edge {
        from: root,
        to: win,
        kind: EdgeKind::Fork,
    });

    let dot = graph.to_dot();

    // The label carries the tier numeral, the score, the first summary line, and the
    // changed-file count that renders S1's `changed_paths`.
    assert!(
        dot.contains("· V · 0.95"),
        "missing tier/score label:\n{dot}"
    );
    assert!(dot.contains("land the fix"));
    assert!(dot.contains("(2 files)"));
    // Tier ramp colours the fill; the Replicator winner is green.
    assert!(dot.contains("fillcolor=\"#2ea043\""));
    // The best-score lineage (win → root) is the kept path, gold-bordered on both
    // the nodes and the edge between them.
    assert_eq!(
        dot.matches("color=\"#d4af37\"").count(),
        3,
        "two kept nodes and one kept edge expected:\n{dot}"
    );
}

#[test]
fn to_dot_escapes_untrusted_summary_text() {
    let mut graph = Graph::default();
    graph.add_node(
        NodeId::new("n0"),
        Node {
            commit: CommitRef("c0".into()),
            value: None,
            status: NodeStatus::Open,
            parents: vec![],
            substrate: SubstrateState::default(),
            // A hostile summary: embedded quotes, a backslash, a tab, then a newline
            // trailing a forged node statement — the multi-line injection a graph view
            // must neutralise before the text reaches the DOT.
            summary: "said \"hi\" \\ end\ttail\n  forged [label=\"pwned".into(),
            changed_paths: vec![],
        },
    );

    let dot = graph.to_dot();

    // Quotes and backslashes are escaped so the attribute string can't break out, and
    // the tab collapses to a space so the label never spans columns.
    assert!(
        dot.contains("said \\\"hi\\\" \\\\ end tail"),
        "bad escape:\n{dot}"
    );
    assert!(!dot.contains('\t'), "raw control char leaked into the DOT");

    // Only the first summary line is rendered, so the statement forged after the
    // newline is dropped: a multi-line label can never inject a sibling node.
    assert!(
        !dot.contains("forged [label"),
        "summary spilled past its first line:\n{dot}"
    );
}

#[test]
fn node_label_fields_round_trip_and_default_for_legacy_dags() {
    let mut node = Node::open(CommitRef("c0".into()), vec![NodeId::new("root")]);
    node.summary = "rewrote the parser".into();
    node.changed_paths = vec![PathBuf::from("src/parse.rs"), PathBuf::from("src/lib.rs")];

    let json = serde_json::to_string(&node).expect("serialize");
    let restored: Node = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored.summary, "rewrote the parser");
    assert_eq!(restored.changed_paths.len(), 2);

    // A DAG persisted before Phase 6 carries no `summary`/`changed_paths` keys;
    // `#[serde(default)]` must fill them rather than fail the load.
    let legacy = r#"{ "commit": "c0", "value": null, "status": "Open", "parents": [] }"#;
    let legacy: Node = serde_json::from_str(legacy).expect("legacy node loads");
    assert!(legacy.summary.is_empty());
    assert!(legacy.changed_paths.is_empty());
}
