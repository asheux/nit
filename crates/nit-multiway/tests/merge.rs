//! Phase 4 acceptance — the engine's adjudicated-merge step.
//!
//! These drive `Engine::run_merging` (merge ON) over a real `GitWorldStore` in a
//! temp repo, with the agent seam scripted by the `testing` doubles, and cover the
//! four merge outcomes the frozen contract defines:
//!
//! - **complementary** — two forks touch disjoint files, so git joins them with no
//!   conflict and no oracle; the merged two-file tree out-values each one-file
//!   parent (and the DAG, merge node and all, round-trips through `Graph::save`).
//! - **conflicting** — two forks rewrite the same file divergently, git reports a
//!   conflict, and the oracle adjudicates it into a two-parent node.
//! - **unresolved** — a judge that leaves conflict markers yields a *gated* merge
//!   node, withheld from the frontier; the search records it and continues rather
//!   than aborting (the dead-branch rule).
//! - **unsupported** — a store reporting `merge_supported() == false` degrades to
//!   fork-only, minting no two-parent node at all.
//!
//! Two doubles are defined here rather than in `testing`: the marker-leaving judge
//! and the marker-gating valuer stand in for the production `JudgeMergeOracle` +
//! genome force-gate, which live in nit-tui and so are unreachable from this crate.

mod common;

use std::path::{Path, PathBuf};

use nit_core::GenomeTier;
use nit_multiway::git_store::{GitStoreError, GitWorktree, GitWorldStore};
use nit_multiway::graph::Graph;
use nit_multiway::node::{Node, NodeId, NodeStatus};
use nit_multiway::policy::{Budget, Mood, SearchPolicy};
use nit_multiway::search::{Engine, SearchOutcome, StopReason};
use nit_multiway::testing::{
    FakeError, FakeMergeOracle, FakeTurnExecutor, FileCountValuer, ScriptedTurn,
};
use nit_multiway::traits::{
    Conflicts, MergeOracle, MergeOutcome, ResolvedState, TurnExecutor, TurnStatus, Valuer,
    WorldStore,
};
use nit_multiway::value::Value;

use common::{build_merge_engine, git_present, init_operator_repo, task, ScratchDir};

/// The file both forks rewrite in the conflicting scenarios, so git cannot
/// auto-join it, and the file the failing judge re-stamps with conflict markers.
const CONFLICT_FILE: &str = "seed.txt";

/// One scripted `Edited` turn writing `body` to `file`. No label: these tests value
/// trees by file count, not by the `FakeValuer`'s label seam.
fn edit(file: &str, body: &str) -> ScriptedTurn {
    ScriptedTurn {
        status: TurnStatus::Edited,
        summary: format!("write {file}"),
        writes: vec![(PathBuf::from(file), body.to_owned())],
        label: None,
    }
}

/// Seed `engine` and run the merging search at `max_turns` (k=2, `Explore` — the
/// only mood that joins siblings). The four scenarios differ only in their doubles
/// and budget, so the seed→run flow lives here once; the `Explore` policy admits
/// both forks, and the turn cap stops the loop at the unconditional seed expansion
/// that mints the merge, before the exhausted script is re-driven. A returned `Err`
/// would panic here, which is itself the "a merge is never an engine abort" check.
fn run_merge<W, T, V, M>(
    engine: &mut Engine<W, T, V, M>,
    op: &Path,
    max_turns: usize,
) -> SearchOutcome
where
    W: WorldStore,
    T: TurnExecutor,
    V: Valuer,
    M: MergeOracle,
{
    let policy = SearchPolicy {
        mood: Mood::Explore,
        k: 2,
        budget: Budget {
            max_nodes: 64,
            max_turns,
            max_tokens: None,
        },
    };
    let root = engine.seed(op).expect("seed root");
    engine
        .run_merging(root, &policy, &task())
        .expect("a merge is a search event, never an abort")
}

/// The single two-parent node a merge minted — every scenario expects exactly one,
/// so finding two (or none) is a contract failure.
fn sole_merge_node(graph: &Graph) -> Node {
    let mut joins = graph
        .iter_nodes()
        .filter(|(_, node)| node.parents.len() == 2)
        .map(|(_, node)| node.clone());
    let merged = joins.next().expect("a two-parent merge node");
    assert!(joins.next().is_none(), "more than one merge node");
    merged
}

/// Count `EdgeKind::Merge` edges in the DAG. `Graph` exposes no edge iterator, so
/// this reads the serialized form — the same persisted shape the round-trip checks.
/// Each scenario mints at most one merge, so the total localises the join: a clean
/// or conflicting run has two (one per parent), a fork-only run has none.
fn merge_edge_count(graph: &Graph) -> usize {
    let json = serde_json::to_value(graph).expect("encode graph");
    json["edges"]
        .as_array()
        .map(|edges| {
            edges
                .iter()
                .filter(|&edge| edge["kind"].as_str() == Some("Merge"))
                .count()
        })
        .unwrap_or(0)
}

/// A judge that fails to resolve: it rewrites the conflicted file with git-style
/// bookends instead of a clean tree. Mirrors the production `JudgeMergeOracle`
/// leaving markers in the `a` worktree — the dead merge the engine must gate.
struct LeavesConflictMarkers;

impl MergeOracle for LeavesConflictMarkers {
    type Error = FakeError;

    fn adjudicate(
        &self,
        _base: &Path,
        a: &Path,
        _b: &Path,
        _conflicts: &Conflicts,
    ) -> Result<ResolvedState, Self::Error> {
        let marked = "<<<<<<< ours\nalpha\n=======\nbeta\n>>>>>>> theirs\n";
        std::fs::write(a.join(CONFLICT_FILE), marked)?;
        Ok(ResolvedState {
            tree: a.to_path_buf(),
        })
    }
}

/// Gates any tree whose [`CONFLICT_FILE`] still carries both conflict bookends;
/// every other tree is viable. Stands in for the genome valuer's marker force-gate
/// (which lives in nit-tui), so the engine can be shown gating a dead merge here.
struct MarkerGatingValuer;

impl Valuer for MarkerGatingValuer {
    type Error = FakeError;

    fn value(&self, tree: &Path) -> Result<Value, Self::Error> {
        let text = std::fs::read_to_string(tree.join(CONFLICT_FILE)).unwrap_or_default();
        let gated = text.lines().any(|line| line.starts_with("<<<<<<< "))
            && text.lines().any(|line| line.starts_with(">>>>>>> "));
        Ok(Value {
            gated,
            score: if gated { 0.0 } else { 0.5 },
            tier: if gated {
                GenomeTier::StillLife
            } else {
                GenomeTier::Spaceship
            },
        })
    }
}

/// Wraps a real [`GitWorldStore`] but reports `merge_supported() == false`, the
/// git-too-old degrade the engine honours by skipping the merge entirely. Fork and
/// commit delegate unchanged so the children are still real; the join entry points
/// assert they are never reached once the support probe has said no.
struct ForkOnlyStore(GitWorldStore);

impl WorldStore for ForkOnlyStore {
    type Worktree = GitWorktree;
    type Error = GitStoreError;

    fn snapshot(&self, src: &Path) -> Result<NodeId, Self::Error> {
        self.0.snapshot(src)
    }

    fn fork(&self, parent: &NodeId, k: usize) -> Result<Vec<Self::Worktree>, Self::Error> {
        self.0.fork(parent, k)
    }

    fn commit(&self, worktree: Self::Worktree, message: &str) -> Result<NodeId, Self::Error> {
        self.0.commit(worktree, message)
    }

    fn restore(&self, node: &NodeId) -> Result<Self::Worktree, Self::Error> {
        self.0.restore(node)
    }

    fn merge_supported(&self) -> bool {
        false
    }

    fn merge(&self, _a: &NodeId, _b: &NodeId) -> Result<MergeOutcome, Self::Error> {
        unreachable!("fork-only store: the engine must not call merge when unsupported")
    }

    fn commit_merge(
        &self,
        _worktree: Self::Worktree,
        _a: &NodeId,
        _b: &NodeId,
        _message: &str,
    ) -> Result<NodeId, Self::Error> {
        unreachable!("fork-only store: the engine must not call commit_merge when unsupported")
    }
}

#[test]
fn complementary_branches_merge_into_a_higher_value_node() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    let op = ScratchDir::new("merge_clean_op");
    let wt = ScratchDir::new("merge_clean_wt");

    let scripts = vec![
        edit("a.rs", "pub fn a() -> u8 { 1 }\n"),
        edit("b.rs", "pub fn b() -> u8 { 2 }\n"),
    ];
    let mut engine = build_merge_engine(&op, &wt, "merge-clean", scripts);
    let outcome = run_merge(&mut engine, op.path(), 2);
    assert_eq!(outcome.stop_reason, StopReason::BudgetExhausted);

    let merged = sole_merge_node(&engine.graph);
    assert_eq!(merged.parents.len(), 2, "a merge joins exactly two parents");
    assert_eq!(
        merge_edge_count(&engine.graph),
        2,
        "one EdgeKind::Merge edge per parent",
    );

    // The clean join is viable and strictly out-values both single-file parents.
    let merged_value = merged.value.expect("merge node is valued");
    assert!(!merged_value.gated, "a clean merge is viable");
    assert_eq!(merged.status, NodeStatus::Open);
    for parent in &merged.parents {
        let parent_value = engine
            .graph
            .node(parent)
            .and_then(|node| node.value)
            .expect("parent is valued");
        assert!(
            merged_value.score > parent_value.score,
            "merged ({}) must out-score parent ({})",
            merged_value.score,
            parent_value.score,
        );
    }

    // The merge node survives the DAG persistence the runtime relies on: encode,
    // save through a not-yet-existing dir, reload, and re-encode to identical bytes.
    let original = serde_json::to_string(&engine.graph).expect("encode dag");
    let path = op.path().join("out").join("dag.json");
    engine.graph.save(&path).expect("save dag");
    let loaded = Graph::load(&path).expect("load dag");
    assert_eq!(
        original,
        serde_json::to_string(&loaded).expect("re-encode reloaded"),
    );
    let reloaded = sole_merge_node(&loaded);
    assert_eq!(reloaded.parents, merged.parents);
    assert!(reloaded.value.is_some());
}

#[test]
fn conflicting_branches_are_adjudicated_into_a_two_parent_merge() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    let op = ScratchDir::new("merge_conflict_op");
    let wt = ScratchDir::new("merge_conflict_wt");

    let scripts = vec![
        edit(CONFLICT_FILE, "alpha\n"),
        edit(CONFLICT_FILE, "beta\n"),
    ];
    let mut engine = build_merge_engine(&op, &wt, "merge-conflict", scripts);
    let outcome = run_merge(&mut engine, op.path(), 3);

    let merged = sole_merge_node(&engine.graph);
    assert_eq!(merged.parents.len(), 2);
    assert_eq!(merge_edge_count(&engine.graph), 2);
    assert!(merged.value.is_some(), "the adjudicated merge is valued");

    // The third turn is the judge: only the Conflict path charges a turn and mints a
    // two-parent node, so the count is proof the clean path was not taken.
    assert_eq!(outcome.turns_spent, 3);
    assert_eq!(outcome.stop_reason, StopReason::BudgetExhausted);
}

#[test]
fn unresolved_conflict_markers_gate_the_merge_without_aborting() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    let op = ScratchDir::new("merge_marker_op");
    let wt = ScratchDir::new("merge_marker_wt");
    init_operator_repo(op.path());
    let store = GitWorldStore::new(op.path(), wt.path(), "merge-marker").expect("construct store");

    let scripts = vec![
        edit(CONFLICT_FILE, "alpha\n"),
        edit(CONFLICT_FILE, "beta\n"),
    ];
    let mut engine = Engine::new(
        store,
        FakeTurnExecutor::new(scripts),
        MarkerGatingValuer,
        LeavesConflictMarkers,
    );
    // run_merge's `expect` is the no-abort assertion: a gated merge returns Ok.
    let outcome = run_merge(&mut engine, op.path(), 3);

    let merged = sole_merge_node(&engine.graph);
    assert_eq!(merged.parents.len(), 2);
    assert_eq!(merge_edge_count(&engine.graph), 2);
    // Gated and withheld: recorded GateFailed (so never pushed to the frontier),
    // and the search ran on to exhaust its budget rather than failing.
    assert_eq!(merged.status, NodeStatus::GateFailed);
    assert!(merged.value.expect("merge node is valued").gated);
    assert_eq!(outcome.stop_reason, StopReason::BudgetExhausted);
}

#[test]
fn unsupported_merge_degrades_to_fork_only() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    let op = ScratchDir::new("merge_degrade_op");
    let wt = ScratchDir::new("merge_degrade_wt");
    init_operator_repo(op.path());
    let store =
        ForkOnlyStore(GitWorldStore::new(op.path(), wt.path(), "merge-degrade").expect("store"));

    // The same disjoint-file scenario that joins when merge is supported; here the
    // store reports it unsupported, so the engine must skip the merge.
    let scripts = vec![
        edit("a.rs", "pub fn a() {}\n"),
        edit("b.rs", "pub fn b() {}\n"),
    ];
    let oracle = FakeMergeOracle::new(ResolvedState {
        tree: op.path().to_path_buf(),
    });
    let mut engine = Engine::new(
        store,
        FakeTurnExecutor::new(scripts),
        FileCountValuer::new(),
        oracle,
    );
    run_merge(&mut engine, op.path(), 2);

    // Fork-only: no node joined two parents, no Merge edge, just root + two children.
    assert!(
        engine
            .graph
            .iter_nodes()
            .all(|(_, node)| node.parents.len() <= 1),
        "merge_supported()==false must not mint a two-parent node",
    );
    assert_eq!(
        merge_edge_count(&engine.graph),
        0,
        "no merge edge when degraded"
    );
    assert_eq!(engine.graph.len(), 3, "root plus two fork children");
}
