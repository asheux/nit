//! Construction + serde round-trip cover for the Phase 6/7 multiway seams,
//! exercised through the public `nit_core::` re-export surface the 6b popup and
//! the Phase-7 router read. Field accesses are by name, so a frozen-seam rename
//! breaks the build rather than silently passing.

use nit_core::{
    GenomeTier, MultiwayHeader, MultiwayNodeStatus, MultiwayNodeView, MultiwaySource, MultiwayView,
    PendingMultiway,
};

/// A view with a seed root, a solution leaf, and a two-parent merge node so the
/// round-trip exercises every optional field and the multi-parent path.
fn sample_view() -> MultiwayView {
    let root = MultiwayNodeView {
        id: "root0000feedface".to_string(),
        short_id: "root0000".to_string(),
        depth: 0,
        status: MultiwayNodeStatus::Expanded,
        score: Some(0.42),
        tier: Some(GenomeTier::Spaceship),
        summary: "seed commit".to_string(),
        changed: 0,
        parents: Vec::new(),
    };
    let solution = MultiwayNodeView {
        id: "c0ffee01beadcafe".to_string(),
        short_id: "c0ffee01".to_string(),
        depth: 1,
        status: MultiwayNodeStatus::Solution,
        score: Some(0.97),
        tier: Some(GenomeTier::Replicator),
        summary: "passes every gate".to_string(),
        changed: 3,
        parents: vec!["root0000".to_string()],
    };
    let merge = MultiwayNodeView {
        id: "merge999deadbeef0".to_string(),
        short_id: "merge999".to_string(),
        depth: 2,
        status: MultiwayNodeStatus::Held,
        score: None,
        tier: None,
        summary: "merge of two branches".to_string(),
        changed: 1,
        parents: vec!["root0000".to_string(), "c0ffee01".to_string()],
    };
    MultiwayView {
        header: MultiwayHeader {
            mood: "balanced".to_string(),
            k: 3,
            turns: 7,
            max_turns: 64,
            nodes: 3,
            max_nodes: 128,
            frontier: 1,
            best_score: 0.97,
            best_tier: GenomeTier::Replicator,
            stop_reason: Some("solution".to_string()),
        },
        nodes: vec![root, solution, merge],
        kept_path: vec!["root0000".to_string(), "c0ffee01".to_string()],
        in_flight: Some("c0ffee01".to_string()),
    }
}

#[test]
fn multiway_view_round_trips_through_serde() {
    let view = sample_view();
    let json = serde_json::to_string(&view).expect("serialize MultiwayView");
    let back: MultiwayView = serde_json::from_str(&json).expect("deserialize MultiwayView");

    assert_eq!(back.nodes.len(), 3);
    assert_eq!(back.header.mood, "balanced");
    assert_eq!(back.header.best_tier, GenomeTier::Replicator);
    assert_eq!(back.kept_path, view.kept_path);
    assert_eq!(back.in_flight.as_deref(), Some("c0ffee01"));
    assert_eq!(back.nodes[1].status, MultiwayNodeStatus::Solution);
    assert_eq!(back.nodes[1].tier, Some(GenomeTier::Replicator));
    assert_eq!(back.nodes[2].status, MultiwayNodeStatus::Held);
    assert_eq!(back.nodes[2].parents.len(), 2);
    assert_eq!(back.nodes[2].score, None);

    // Re-encoding the decoded value reproduces the bytes: the projection is lossless.
    assert_eq!(serde_json::to_string(&back).expect("re-serialize"), json);
}

#[test]
fn pending_multiway_round_trips_every_source() {
    for source in [
        MultiwaySource::Explicit,
        MultiwaySource::Shadow,
        MultiwaySource::Swarm,
        MultiwaySource::All,
        MultiwaySource::Bare,
    ] {
        let pending = PendingMultiway {
            source,
            command: "@multiway fix the parser".to_string(),
        };
        let json = serde_json::to_string(&pending).expect("serialize PendingMultiway");
        let back: PendingMultiway =
            serde_json::from_str(&json).expect("deserialize PendingMultiway");
        assert_eq!(back, pending);
    }
}
