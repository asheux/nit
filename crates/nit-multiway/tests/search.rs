//! Phase 2 acceptance — the search loop's stop conditions.
//!
//! Each test drives `Engine::run` over a `LabelKeyedExecutor` fixture against a
//! real `GitWorldStore` and pins one exit of the loop: an early `Solution`, an
//! exhausted `Budget`, and a drained frontier. The last test asserts the invariant
//! a gate exists to enforce — a gate-failed child is recorded in the DAG for
//! provenance but is never admitted to the frontier nor chosen as the best node.

mod common;

use nit_core::GenomeTier;
use nit_multiway::node::NodeStatus;
use nit_multiway::policy::{Budget, Mood, SearchPolicy};
use nit_multiway::search::StopReason;
use nit_multiway::testing::FakeValuer;

use common::{build_engine, gated, git_present, task, viable, ScratchDir};

fn budget(max_nodes: usize, max_turns: usize) -> Budget {
    Budget {
        max_nodes,
        max_turns,
        max_tokens: None,
    }
}

fn gate_failed_count(engine: &common::FakeEngine) -> usize {
    engine
        .graph
        .iter_nodes()
        .filter(|(_, node)| node.status == NodeStatus::GateFailed)
        .count()
}

#[test]
fn stops_on_solution_when_a_replicator_node_is_reached() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    let op = ScratchDir::new("solution_op");
    let wt = ScratchDir::new("solution_wt");
    let children: &[(&str, &[&str])] = &[("", &["win", "ok"])];
    let valuer = FakeValuer::new(viable(0.0, GenomeTier::StillLife))
        .with_label("win", viable(0.95, GenomeTier::Replicator))
        .with_label("ok", viable(0.50, GenomeTier::Spaceship));

    let mut engine = build_engine(&op, &wt, "solution", children, valuer);
    let policy = SearchPolicy {
        mood: Mood::Explore,
        k: 2,
        budget: budget(64, 16),
    };
    let root = engine.seed(op.path()).expect("seed");
    let outcome = engine.run(root, &policy, &task()).expect("run");

    assert_eq!(outcome.stop_reason, StopReason::Solution);
    let best = engine
        .graph
        .node(&outcome.best.expect("best"))
        .expect("node")
        .value
        .expect("valued");
    assert_eq!(best.tier, GenomeTier::Replicator);
    // The accepted terminal is marked `Solution` in the DAG, not merely returned.
    assert!(engine
        .graph
        .iter_nodes()
        .any(|(_, node)| node.status == NodeStatus::Solution));
}

#[test]
fn stops_on_budget_exhausted_when_the_turn_cap_binds() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    let op = ScratchDir::new("budget_op");
    let wt = ScratchDir::new("budget_wt");
    // An endless Spaceship plateau: never a Replicator (no early `Solution`) and
    // never empty (every expansion re-admits a `step`), so only the budget stops it.
    let children: &[(&str, &[&str])] = &[("", &["step", "step"]), ("step", &["step", "step"])];
    let valuer = FakeValuer::new(viable(0.0, GenomeTier::StillLife))
        .with_label("step", viable(0.50, GenomeTier::Spaceship));

    let mut engine = build_engine(&op, &wt, "budget", children, valuer);
    let policy = SearchPolicy {
        mood: Mood::Exploit,
        k: 2,
        budget: budget(256, 6),
    };
    let root = engine.seed(op.path()).expect("seed");
    let outcome = engine.run(root, &policy, &task()).expect("run");

    assert_eq!(outcome.stop_reason, StopReason::BudgetExhausted);
    assert!(
        outcome.turns_spent >= policy.budget.max_turns,
        "the turn cap, not the node cap, must bind ({} turns)",
        outcome.turns_spent
    );
}

#[test]
fn stops_on_frontier_empty_when_the_seed_has_no_viable_child() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    let op = ScratchDir::new("empty_op");
    let wt = ScratchDir::new("empty_wt");
    let children: &[(&str, &[&str])] = &[("", &["bad", "bad"])];
    let valuer = FakeValuer::new(viable(0.0, GenomeTier::StillLife)).with_label("bad", gated());

    let mut engine = build_engine(&op, &wt, "empty", children, valuer);
    let policy = SearchPolicy {
        mood: Mood::Explore,
        k: 2,
        budget: budget(64, 16),
    };
    let root = engine.seed(op.path()).expect("seed");
    let outcome = engine.run(root, &policy, &task()).expect("run");

    assert_eq!(outcome.stop_reason, StopReason::FrontierEmpty);
    assert!(outcome.best.is_none(), "no viable node was ever observed");
    assert!(engine.frontier.is_empty());
    // Both gated children are still recorded in the DAG for provenance.
    assert_eq!(gate_failed_count(&engine), 2);
}

#[test]
fn gate_failed_children_are_recorded_but_withheld_from_the_frontier() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    let op = ScratchDir::new("withheld_op");
    let wt = ScratchDir::new("withheld_wt");
    // The seed yields one viable (`ok`) and one gated (`bad`); expanding `ok` yields
    // two more gated children, so the search drains once the lone viable line ends.
    let children: &[(&str, &[&str])] = &[("", &["ok", "bad"]), ("ok", &["bad", "bad"])];
    let valuer = FakeValuer::new(viable(0.0, GenomeTier::StillLife))
        .with_label("ok", viable(0.50, GenomeTier::Spaceship))
        .with_label("bad", gated());

    let mut engine = build_engine(&op, &wt, "withheld", children, valuer);
    let policy = SearchPolicy {
        mood: Mood::Explore,
        k: 2,
        budget: budget(64, 16),
    };
    let root = engine.seed(op.path()).expect("seed");
    let outcome = engine.run(root, &policy, &task()).expect("run");

    assert_eq!(outcome.stop_reason, StopReason::FrontierEmpty);
    // The best node is the only viable one — a gated child never wins despite being
    // in the DAG.
    let best = engine
        .graph
        .node(&outcome.best.expect("a viable best node"))
        .expect("node")
        .value
        .expect("valued");
    assert_eq!(best.tier, GenomeTier::Spaceship);
    assert!(!best.gated);
    // Three gate-failed children recorded (one at the seed, two under `ok`), and the
    // frontier is empty — none of them was ever admitted.
    assert_eq!(gate_failed_count(&engine), 3);
    assert!(engine.frontier.is_empty());
}
