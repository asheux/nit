//! Phase 6 engine-prereq acceptance — the streaming observer hook.
//!
//! Two invariants the live graph view depends on: the observer fires **exactly
//! once per expansion** (so a run's observer-call count equals its
//! `nodes_expanded`), and the no-op-default delegation leaves `run` /
//! `run_merging` driving the **identical** search — observing surfaces progress
//! without changing the outcome. Commit SHAs are timestamp-derived, so two
//! independent runs never share `NodeId`s; the equivalence checks compare a
//! SHA-independent structural fingerprint (status · tier · score · arity) plus the
//! scalar outcome, not the raw DAG.

mod common;

use nit_core::GenomeTier;
use nit_multiway::frontier::Frontier;
use nit_multiway::graph::Graph;
use nit_multiway::policy::{Budget, Mood, SearchPolicy};
use nit_multiway::search::{RunSnapshot, SearchOutcome};
use nit_multiway::testing::{FakeValuer, ScriptedTurn};
use nit_multiway::traits::TurnStatus;

use common::{build_engine, build_merge_engine, git_present, task, viable, ScratchDir};

use std::path::PathBuf;

/// A self-replicating Spaceship plateau: every expansion re-admits a `step`, so
/// only the turn budget stops the search and it runs several expansions — enough
/// for the once-per-expand count to be meaningful.
const PLATEAU: &[(&str, &[&str])] = &[("", &["step", "step"]), ("step", &["step", "step"])];

fn plateau_valuer() -> FakeValuer {
    FakeValuer::new(viable(0.0, GenomeTier::StillLife))
        .with_label("step", viable(0.50, GenomeTier::Spaceship))
}

fn plateau_policy() -> SearchPolicy {
    SearchPolicy {
        mood: Mood::Exploit,
        k: 2,
        budget: Budget {
            max_nodes: 256,
            max_turns: 6,
            max_tokens: None,
        },
    }
}

/// A SHA-free structural fingerprint of the DAG: a sorted multiset of each node's
/// status, value (tier · score · gated) and parent arity. Two runs of the same
/// fixture differ only in their commit SHAs, which this deliberately omits, so an
/// equal fingerprint proves the searches were structurally identical.
fn fingerprint(graph: &Graph) -> Vec<String> {
    let mut rows: Vec<String> = graph
        .iter_nodes()
        .map(|(_, node)| {
            let value = node.value.map_or_else(
                || "none".to_owned(),
                |v| format!("{}|{:.4}|{}", v.tier.numeral(), v.score, v.gated),
            );
            format!("{:?}|{}|{}", node.status, value, node.parents.len())
        })
        .collect();
    rows.sort();
    rows
}

/// The tier and score of the outcome's best node, looked up in that run's own
/// graph — comparable across runs even though the `NodeId` keying it is not.
fn best_value(graph: &Graph, outcome: &SearchOutcome) -> Option<(GenomeTier, f32)> {
    outcome
        .best
        .as_ref()
        .and_then(|id| graph.node(id))
        .and_then(|node| node.value)
        .map(|v| (v.tier, v.score))
}

#[test]
fn observer_fires_once_per_expansion() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    let op = ScratchDir::new("observer_count_op");
    let wt = ScratchDir::new("observer_count_wt");
    let mut engine = build_engine(&op, &wt, "observer-count", PLATEAU, plateau_valuer());
    let policy = plateau_policy();
    let root = engine.seed(op.path()).expect("seed");

    let mut fires = 0usize;
    let mut steps: Vec<(usize, usize)> = Vec::new();
    let outcome = engine
        .run_observed(
            root,
            &policy,
            &task(),
            &mut |graph: &Graph, _frontier: &Frontier, snap: &RunSnapshot| {
                fires += 1;
                steps.push((graph.len(), snap.nodes_expanded));
            },
        )
        .expect("run_observed");

    // One fire per expand — the seed expansion plus every loop step — so the count
    // is exactly the reported expansion total, and the plateau guarantees > 1.
    assert_eq!(fires, outcome.nodes_expanded);
    assert!(fires > 1, "the plateau fixture must expand more than once");

    // Each fire follows one expand, so the snapshot's `nodes_expanded` advances by
    // exactly one between consecutive observations and the graph never shrinks.
    assert!(steps
        .windows(2)
        .all(|w| w[1].1 == w[0].1 + 1 && w[1].0 >= w[0].0));
    assert_eq!(steps.last().map(|s| s.1), Some(outcome.nodes_expanded));
}

#[test]
fn run_observed_with_a_noop_matches_run() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    let policy = plateau_policy();

    let plain = {
        let op = ScratchDir::new("equiv_plain_op");
        let wt = ScratchDir::new("equiv_plain_wt");
        let mut engine = build_engine(&op, &wt, "equiv-plain", PLATEAU, plateau_valuer());
        let root = engine.seed(op.path()).expect("seed");
        let outcome = engine.run(root, &policy, &task()).expect("run");
        (
            outcome.clone(),
            fingerprint(&engine.graph),
            best_value(&engine.graph, &outcome),
        )
    };

    let observed = {
        let op = ScratchDir::new("equiv_obs_op");
        let wt = ScratchDir::new("equiv_obs_wt");
        let mut engine = build_engine(&op, &wt, "equiv-obs", PLATEAU, plateau_valuer());
        let root = engine.seed(op.path()).expect("seed");
        let outcome = engine
            .run_observed(root, &policy, &task(), &mut |_, _, _| {})
            .expect("run_observed");
        (
            outcome.clone(),
            fingerprint(&engine.graph),
            best_value(&engine.graph, &outcome),
        )
    };

    // Same scalar outcome, same structural DAG, same best value: the no-op observer
    // changed nothing about the search it streamed.
    assert_eq!(plain.0.nodes_expanded, observed.0.nodes_expanded);
    assert_eq!(plain.0.turns_spent, observed.0.turns_spent);
    assert_eq!(plain.0.stop_reason, observed.0.stop_reason);
    assert_eq!(plain.1, observed.1, "structural fingerprints diverged");
    assert_eq!(plain.2, observed.2, "best-node value diverged");
}

/// One scripted `Edited` turn writing a trivial module — disjoint files across the
/// two forks give the clean two-parent merge this equivalence check exercises.
fn merge_scripts() -> Vec<ScriptedTurn> {
    let write = |file: &str| ScriptedTurn {
        status: TurnStatus::Edited,
        summary: format!("write {file}"),
        writes: vec![(PathBuf::from(file), "pub fn unit() {}\n".to_owned())],
        label: None,
    };
    vec![write("a.rs"), write("b.rs")]
}

#[test]
fn run_merging_observed_with_a_noop_matches_run_merging() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    let policy = SearchPolicy {
        mood: Mood::Explore,
        k: 2,
        budget: Budget {
            max_nodes: 64,
            max_turns: 2,
            max_tokens: None,
        },
    };

    let plain = {
        let op = ScratchDir::new("equiv_merge_plain_op");
        let wt = ScratchDir::new("equiv_merge_plain_wt");
        let mut engine = build_merge_engine(&op, &wt, "equiv-merge-plain", merge_scripts());
        let root = engine.seed(op.path()).expect("seed");
        let outcome = engine
            .run_merging(root, &policy, &task())
            .expect("run_merging");
        (outcome.stop_reason, fingerprint(&engine.graph))
    };

    let observed = {
        let op = ScratchDir::new("equiv_merge_obs_op");
        let wt = ScratchDir::new("equiv_merge_obs_wt");
        let mut engine = build_merge_engine(&op, &wt, "equiv-merge-obs", merge_scripts());
        let root = engine.seed(op.path()).expect("seed");
        let outcome = engine
            .run_merging_observed(root, &policy, &task(), &mut |_, _, _| {})
            .expect("run_merging_observed");
        (outcome.stop_reason, fingerprint(&engine.graph))
    };

    // The merge path delegates the same way: a two-parent join appears identically
    // whether or not an observer watched it form.
    assert_eq!(plain.0, observed.0);
    assert_eq!(plain.1, observed.1, "merge-path fingerprints diverged");
    assert!(
        plain.1.iter().any(|row| row.ends_with("|2")),
        "the fixture must mint a two-parent merge node",
    );
}
