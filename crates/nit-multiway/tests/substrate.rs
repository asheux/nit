//! Phase-4 per-branch `substrate` coverage. The serde round-trips prove a
//! populated snapshot survives a serialize/deserialize cycle and a pre-Phase-4
//! node (no `substrate` key) still loads via `#[serde(default)]`; the merge-node
//! test proves the engine reconciles its two parents' substrate onto the join.

mod common;

use std::path::PathBuf;

use nit_core::{Claim, ClaimKind, ClaimTarget, GenomeTier, SubstrateState};
use nit_multiway::node::{CommitRef, Node, NodeId, NodeStatus};
use nit_multiway::policy::{Budget, Mood, SearchPolicy};
use nit_multiway::testing::ScriptedTurn;
use nit_multiway::traits::TurnStatus;
use nit_multiway::Value;

use common::{build_merge_engine, git_present, task, ScratchDir};

fn substrate_with_one_claim() -> SubstrateState {
    let mut s = SubstrateState::new();
    s.generation = 7;
    s.signal_counter = 3;
    s.claim_counter = 2;
    let claim = Claim {
        id: "7-agent-a-1".into(),
        kind: ClaimKind::ExclusiveWrite,
        target: ClaimTarget::File {
            path: PathBuf::from("src/lib.rs"),
        },
        claimed_by: "agent-a".into(),
        claimed_at_gen: 7,
        ttl_gens: 5,
        rationale: "owns the module".into(),
    };
    s.claims.insert(claim.id.clone(), claim);
    s
}

#[test]
fn node_round_trips_with_its_substrate_snapshot() {
    let node = Node::child(
        CommitRef("c1".into()),
        Value {
            gated: false,
            score: 0.61,
            tier: GenomeTier::Methuselah,
        },
        NodeStatus::Open,
        NodeId::new("root"),
        substrate_with_one_claim(),
    );

    let json = serde_json::to_string(&node).expect("serialize");
    let restored: Node = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(restored.substrate.generation, 7);
    assert_eq!(restored.substrate.signal_counter, 3);
    assert_eq!(restored.substrate.claim_counter, 2);
    let claim = restored
        .substrate
        .claims
        .get("7-agent-a-1")
        .expect("claim survived the round-trip");
    assert_eq!(claim.kind, ClaimKind::ExclusiveWrite);
    assert_eq!(restored.parents, vec![NodeId::new("root")]);
}

#[test]
fn pre_phase4_node_without_substrate_key_loads_as_default() {
    // A DAG persisted before Phase 4 has no `substrate` field; `#[serde(default)]`
    // must fill it rather than fail the load.
    let legacy = r#"{
        "commit": "c0",
        "value": null,
        "status": "Open",
        "parents": []
    }"#;

    let node: Node = serde_json::from_str(legacy).expect("legacy node loads");
    assert_eq!(node.substrate.generation, 0);
    assert!(node.substrate.claims.is_empty());
    assert!(node.substrate.signals.is_empty());
    assert!(node.substrate.assumptions.is_empty());
}

#[test]
fn merge_node_carries_the_reconciled_substrate_of_its_parents() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    let op = ScratchDir::new("substrate_merge_op");
    let wt = ScratchDir::new("substrate_merge_wt");

    // A clean disjoint-file merge so the engine mints a two-parent node and runs
    // `SubstrateState::reconcile` over the parents' captured snapshots.
    let scripts = vec![scripted_write("a.rs"), scripted_write("b.rs")];
    let mut engine = build_merge_engine(&op, &wt, "substrate-merge", scripts);
    let policy = SearchPolicy {
        mood: Mood::Explore,
        k: 2,
        budget: Budget {
            max_nodes: 64,
            max_turns: 2,
            max_tokens: None,
        },
    };
    let root = engine.seed(op.path()).expect("seed root");
    engine
        .run_merging(root, &policy, &task())
        .expect("run merging");

    let merged = engine
        .graph
        .iter_nodes()
        .find(|(_, node)| node.parents.len() == 2)
        .map(|(_, node)| node.clone())
        .expect("a two-parent merge node");

    // The join carries `reconcile(parent_a, parent_b)` in the parents' canonical
    // order — `a` is the higher-scored sibling, recorded as `parents[0]`. reconcile
    // is a-biased (not commutative), so the engine and this independent recompute
    // must agree on order; both use `[parents[0], parents[1]]`, which is why no
    // order-independence is asserted (the frozen contract does not provide it).
    let parent_a = engine
        .graph
        .node(&merged.parents[0])
        .expect("parent a present");
    let parent_b = engine
        .graph
        .node(&merged.parents[1])
        .expect("parent b present");
    let expected = SubstrateState::reconcile(&parent_a.substrate, &parent_b.substrate);
    assert_eq!(
        serde_json::to_value(&merged.substrate).expect("encode merged substrate"),
        serde_json::to_value(&expected).expect("encode reconciled substrate"),
        "the merge node must carry reconcile(parent_a, parent_b)",
    );

    // v1 mints no per-branch substrate, so the reconciled snapshot is the default;
    // the equality above is what will catch a v2 regression once branches diverge.
    assert_eq!(merged.substrate.generation, 0);
    assert!(merged.substrate.claims.is_empty());
    assert!(merged.substrate.signals.is_empty());
}

/// One scripted `Edited` turn writing a trivial module to `file` — disjoint files
/// across the two forks give the clean merge this test needs.
fn scripted_write(file: &str) -> ScriptedTurn {
    ScriptedTurn {
        status: TurnStatus::Edited,
        summary: format!("write {file}"),
        writes: vec![(PathBuf::from(file), "pub fn unit() {}\n".to_owned())],
        label: None,
    }
}
