//! Phase 2 acceptance — the persisted DAG round-trips byte-for-byte.
//!
//! `tests/graph.rs` round-trips a hand-built two-node graph; this round-trips the
//! DAG a *real search* emits — populated `Some(Value)` scores, `Fork` edges, and
//! the spread of statuses the loop assigns (Expanded / Open / GateFailed /
//! Solution). Encoding it, persisting through `Graph::{save,load}`, and re-encoding
//! the restored graph must reproduce identical bytes; the `BTreeMap` node ordering
//! is what makes that deterministic regardless of insertion order.

mod common;

use nit_core::GenomeTier;
use nit_multiway::graph::Graph;
use nit_multiway::node::NodeStatus;
use nit_multiway::policy::{Budget, Mood, SearchPolicy};
use nit_multiway::search::StopReason;
use nit_multiway::testing::FakeValuer;

use common::{build_engine, gated, git_present, task, viable, ScratchDir};

#[test]
fn a_searched_dag_round_trips_byte_for_byte_through_save_and_load() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    let op = ScratchDir::new("serde_op");
    let wt = ScratchDir::new("serde_wt");

    // Root forks a viable `hi` and a gated `bad`; `hi` forks the `win` Replicator
    // (accepted as the solution) and a lower `low` (left Open on the frontier). The
    // result exercises Expanded, Open, GateFailed and Solution nodes plus Fork edges.
    let children: &[(&str, &[&str])] = &[("", &["hi", "bad"]), ("hi", &["win", "low"])];
    let valuer = FakeValuer::new(viable(0.0, GenomeTier::StillLife))
        .with_label("hi", viable(0.80, GenomeTier::Methuselah))
        .with_label("bad", gated())
        .with_label("win", viable(0.95, GenomeTier::Replicator))
        .with_label("low", viable(0.30, GenomeTier::Spaceship));

    let mut engine = build_engine(&op, &wt, "serde", children, valuer);
    let policy = SearchPolicy {
        mood: Mood::Explore,
        k: 2,
        budget: Budget {
            max_nodes: 64,
            max_turns: 16,
            max_tokens: None,
        },
    };
    let root = engine.seed(op.path()).expect("seed");
    let outcome = engine.run(root, &policy, &task()).expect("run");

    // A non-trivial, real search output: root + hi + bad + win + low.
    assert_eq!(outcome.stop_reason, StopReason::Solution);
    assert!(
        engine.graph.len() >= 5,
        "expected a multi-node searched DAG"
    );

    let original = serde_json::to_string(&engine.graph).expect("encode the searched dag");

    // File round-trip into a path whose parent dir does not exist yet, proving
    // `save` creates parents and writes atomically.
    let path = op.path().join("out").join("dag.json");
    engine.graph.save(&path).expect("save dag");
    let loaded = Graph::load(&path).expect("load dag");

    // Byte-stable: the restored graph re-encodes to the exact original bytes.
    assert_eq!(
        original,
        serde_json::to_string(&loaded).expect("re-encode restored"),
    );
    assert_eq!(loaded.len(), engine.graph.len());

    // Populated `Some(Value)` survives the trip — the gap `tests/graph.rs` leaves,
    // asserted here on real search output: the accepted Replicator comes back whole.
    let win = loaded
        .node(&outcome.best.expect("best node"))
        .expect("solution node present");
    assert_eq!(win.status, NodeStatus::Solution);
    let value = win.value.expect("solution node is valued");
    assert_eq!(value.tier, GenomeTier::Replicator);
    assert!(!value.gated);
    // And a gate-failed node persisted too, so every status the loop emits round-trips.
    assert!(loaded
        .iter_nodes()
        .any(|(_, node)| node.status == NodeStatus::GateFailed));
}
