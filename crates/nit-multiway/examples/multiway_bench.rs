//! Phase 5 measurement harness — does multiway + backtracking beat the linear
//! single-writer at a *matched* token/turn budget? (`docs/MULTIWAY.md` Phase 5.)
//!
//!   cargo run -p nit-multiway --example multiway_bench -- [out.json]
//!
//! It runs a FIXED task set both ways on the pure engine with the deterministic
//! `testing` doubles — a real `GitWorldStore` over a throwaway temp repo, a
//! `LabelKeyedExecutor` scripting the search tree, and a label-keyed `FakeValuer`
//! — so the comparison reproduces without spawning a single agent. Two arms share
//! one `Budget` per scenario (a `Copy` value handed to both, so "matched budget"
//! is a property of the code, not a number to eyeball):
//!   * LINEAR   = `SearchPolicy { mood: Exploit, k: 1 }` — the greedy single-path
//!     analog of today's linear single-writer.
//!   * MULTIWAY = `SearchPolicy { mood: Explore, k: 2 }` — the wide frontier whose
//!     held siblings let `Frontier::pop_best` backtrack when a branch regresses.
//!
//! Both arms call `Engine::run` (merge OFF): Phase 5's headline is the
//! frontier+backtrack advantage, and an adjudicated merge on this fake stack is
//! degenerate (`LabelKeyedExecutor` rewrites the same files every turn, so every
//! sibling join would conflict). A merge-on column waits on the real-agent arm.
//!
//! Task set — each scenario is one labelled expansion tree plus a label→`Value`
//! map; a label with no children entry is a leaf the search never expands:
//!   * `trap` — a local-optimum trap (the `tests/trap.rs` fixture): a high
//!     Methuselah branch whose children plateau, hiding the only path to the
//!     Replicator. The case multiway is built for.
//!   * `smooth` — a monotone ramp, no trap: the locally-best child is also
//!     globally best, so greedy walks straight to the optimum; multiway's extra
//!     forks are wasted budget.
//!   * `starved_trap` — the trap again at half the turn budget, too tight for
//!     multiway to explore-and-backtrack before the budget ends.
//!
//! HONESTY: budgets are matched per scenario and every scenario is reported. A
//! null result (linear ties or beats multiway) is the finding, not something to
//! hide — `smooth` and `starved_trap` tie here, and the summary says so. The
//! reproducible evidence columns are `turns`/`nodes` (the matched-budget cost) and
//! `final_tier`/`final_score` (the outcome). `wall_us` is informational only on
//! this stack: it is dominated by git plumbing and varies run to run, so it is NOT
//! the multiway-vs-linear signal — real wall-clock waits on the real-agent arm.

use std::cmp::Ordering;
use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::Instant;

use nit_core::GenomeTier;
use nit_multiway::git_store::GitWorldStore;
use nit_multiway::policy::{Budget, Mood, SearchPolicy};
use nit_multiway::search::{Engine, StopReason};
use nit_multiway::testing::{FakeMergeOracle, FakeValuer, LabelKeyedExecutor};
use nit_multiway::traits::{ResolvedState, Task};
use nit_multiway::Value;
use serde::Serialize;

// The `trap` landscape, mirroring `tests/trap.rs`: root forks into the tempting
// `trunk` and a low `side`; `trunk` forks into the Methuselah local optimum `hi`
// (whose children regress to a self-stamping 0.30 plateau) and the lower-now `lo`,
// the sole path to the `lo_win` Replicator. Greedy commits to `hi` and never
// backtracks to `lo`; a wide frontier keeps `lo` alive and re-selects it.
const TRAP_CHILDREN: &[(&str, &[&str])] = &[
    ("", &["trunk", "side"]),
    ("trunk", &["hi", "lo"]),
    ("hi", &["hi_low", "hi_low"]),
    ("hi_low", &["hi_low", "hi_low"]),
    ("lo", &["lo_win", "lo_meh"]),
];
const TRAP_LABELS: &[(&str, f32, GenomeTier)] = &[
    ("trunk", 0.70, GenomeTier::Spaceship),
    ("side", 0.10, GenomeTier::StillLife),
    ("hi", 0.80, GenomeTier::Methuselah),
    ("lo", 0.40, GenomeTier::Spaceship),
    ("hi_low", 0.30, GenomeTier::Spaceship),
    ("lo_win", 0.95, GenomeTier::Replicator),
    ("lo_meh", 0.30, GenomeTier::Spaceship),
];

// The `smooth` control: a monotone ramp where the first (locally-best) child is
// always the globally-best path, so greedy reaches the Replicator `peak` directly.
const SMOOTH_CHILDREN: &[(&str, &[&str])] = &[
    ("", &["up", "down"]),
    ("up", &["up2", "up_side"]),
    ("up2", &["peak", "up2_side"]),
];
const SMOOTH_LABELS: &[(&str, f32, GenomeTier)] = &[
    ("up", 0.50, GenomeTier::Spaceship),
    ("down", 0.20, GenomeTier::StillLife),
    ("up2", 0.70, GenomeTier::Methuselah),
    ("up_side", 0.30, GenomeTier::Spaceship),
    ("peak", 0.95, GenomeTier::Replicator),
    ("up2_side", 0.40, GenomeTier::Spaceship),
];

struct Scenario {
    name: &'static str,
    blurb: &'static str,
    children: &'static [(&'static str, &'static [&'static str])],
    labels: &'static [(&'static str, f32, GenomeTier)],
    multiway_k: usize,
    budget: Budget,
}

/// The fixed task set. `wide` lets multiway explore and backtrack; `starved` is
/// the same trap with too few turns for the backtrack to pay off. Both arms of a
/// scenario receive the scenario's single `budget`, so the budgets are matched by
/// construction.
fn scenarios() -> Vec<Scenario> {
    let wide = Budget {
        max_nodes: 64,
        max_turns: 12,
        max_tokens: None,
    };
    let starved = Budget {
        max_nodes: 64,
        max_turns: 6,
        max_tokens: None,
    };
    vec![
        Scenario {
            name: "trap",
            blurb: "local-optimum trap, wide budget — the case multiway is built for",
            children: TRAP_CHILDREN,
            labels: TRAP_LABELS,
            multiway_k: 2,
            budget: wide,
        },
        Scenario {
            name: "smooth",
            blurb: "monotone ramp, no trap — greedy already reaches the global optimum",
            children: SMOOTH_CHILDREN,
            labels: SMOOTH_LABELS,
            multiway_k: 2,
            budget: wide,
        },
        Scenario {
            name: "starved_trap",
            blurb: "same trap, half the budget — too tight for multiway to backtrack in time",
            children: TRAP_CHILDREN,
            labels: TRAP_LABELS,
            multiway_k: 2,
            budget: starved,
        },
    ]
}

/// Metrics from one arm of one scenario. `score`/`tier` are the best valued node
/// the search settled on; `turns`/`nodes` are the budget cost it paid to get there.
struct ArmResult {
    reached_solution: bool,
    tier: GenomeTier,
    score: f32,
    turns: usize,
    nodes: usize,
    stop_reason: StopReason,
    wall_us: u128,
}

struct ScenarioResult {
    name: &'static str,
    blurb: &'static str,
    budget: Budget,
    linear: ArmResult,
    multiway: ArmResult,
    winner: &'static str,
}

/// One serializable output record (`scenario × arm`); the `winner` column repeats
/// the scenario's verdict on both of its rows so a single flat table is enough.
#[derive(Serialize)]
struct Row {
    scenario: &'static str,
    arm: &'static str,
    reached_solution: bool,
    final_tier: GenomeTier,
    final_score: f64,
    turns: usize,
    nodes: usize,
    stop_reason: StopReason,
    budget_turns: usize,
    budget_nodes: usize,
    wall_us: u128,
    winner: &'static str,
}

fn main() -> Result<(), Box<dyn Error>> {
    if !git_present() {
        eprintln!("skipping multiway_bench: git not found on PATH");
        return Ok(());
    }
    let out_path = std::env::args().nth(1).map(PathBuf::from);

    let mut results = Vec::new();
    for sc in &scenarios() {
        let linear = run_arm(sc, "linear", Mood::Exploit, 1)?;
        let multiway = run_arm(sc, "multiway", Mood::Explore, sc.multiway_k)?;
        let winner = decide_winner(&linear, &multiway);
        results.push(ScenarioResult {
            name: sc.name,
            blurb: sc.blurb,
            budget: sc.budget,
            linear,
            multiway,
            winner,
        });
    }

    let rows = to_rows(&results);
    let csv = emit_csv(&rows);
    let json = serde_json::to_string_pretty(&rows)?;

    print!("{csv}");
    print_summary(&results);

    match out_path {
        Some(path) => {
            std::fs::write(&path, &json)?;
            println!(
                "\nwrote JSON report ({} rows) to {}",
                rows.len(),
                path.display()
            );
        }
        None => println!("\n--- JSON ---\n{json}"),
    }
    Ok(())
}

/// Run one arm over a fresh temp repo and report its metrics. Each call gets its
/// own operator + worktrees scratch dirs so the six runs never share git state;
/// the engine (and its store's worktrees/refs) is dropped before the dirs are.
fn run_arm(sc: &Scenario, arm: &str, mood: Mood, k: usize) -> Result<ArmResult, Box<dyn Error>> {
    let op = ScratchDir::new("op");
    let wt = ScratchDir::new("wt");
    init_operator_repo(op.path())?;

    let mission = format!("bench-{}-{arm}", sc.name);
    let store = GitWorldStore::new(op.path(), wt.path(), &mission)?;
    let oracle = FakeMergeOracle::new(ResolvedState {
        tree: op.path().to_path_buf(),
    });
    let mut engine = Engine::new(
        store,
        LabelKeyedExecutor::new(sc.children),
        build_valuer(sc),
        oracle,
    );

    let policy = SearchPolicy {
        mood,
        k,
        budget: sc.budget,
    };
    let task = Task {
        prompt: "multiway-bench".to_owned(),
        role: "bench".to_owned(),
    };

    let started = Instant::now();
    let root = engine.seed(op.path())?;
    let outcome = engine.run(root, &policy, &task)?;
    let wall_us = started.elapsed().as_micros();

    // The best node is always a viable, valued child; fall back to a gated
    // StillLife only in the degenerate case where no child was ever committed.
    let value = outcome
        .best
        .as_ref()
        .and_then(|id| engine.graph.node(id))
        .and_then(|node| node.value)
        .unwrap_or(Value {
            gated: true,
            score: 0.0,
            tier: GenomeTier::StillLife,
        });

    Ok(ArmResult {
        reached_solution: outcome.stop_reason == StopReason::Solution,
        tier: value.tier,
        score: value.score,
        turns: outcome.turns_spent,
        nodes: outcome.nodes_expanded,
        stop_reason: outcome.stop_reason,
        wall_us,
    })
}

fn build_valuer(sc: &Scenario) -> FakeValuer {
    let mut valuer = FakeValuer::new(Value {
        gated: false,
        score: 0.0,
        tier: GenomeTier::StillLife,
    });
    for &(label, score, tier) in sc.labels {
        valuer = valuer.with_label(
            label,
            Value {
                gated: false,
                score,
                tier,
            },
        );
    }
    valuer
}

/// Multiway wins iff it ends on a strictly higher tier, or the same tier with a
/// higher score; equal on both is a tie. Scores compare via `total_cmp` (the
/// engine's ordering), never `partial_cmp`.
fn decide_winner(linear: &ArmResult, multiway: &ArmResult) -> &'static str {
    let order = multiway
        .tier
        .cmp(&linear.tier)
        .then_with(|| multiway.score.total_cmp(&linear.score));
    match order {
        Ordering::Greater => "multiway",
        Ordering::Less => "linear",
        Ordering::Equal => "tie",
    }
}

fn to_rows(results: &[ScenarioResult]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(results.len() * 2);
    for r in results {
        rows.push(row(r.name, "linear", &r.linear, r.budget, r.winner));
        rows.push(row(r.name, "multiway", &r.multiway, r.budget, r.winner));
    }
    rows
}

fn row(
    scenario: &'static str,
    arm: &'static str,
    m: &ArmResult,
    budget: Budget,
    winner: &'static str,
) -> Row {
    Row {
        scenario,
        arm,
        reached_solution: m.reached_solution,
        final_tier: m.tier,
        // Round for display only: the scenario scores are deliberate round numbers,
        // and an f64 carries the shortest round-trip repr where a raw f32 prints noise.
        final_score: ((m.score as f64) * 1000.0).round() / 1000.0,
        turns: m.turns,
        nodes: m.nodes,
        stop_reason: m.stop_reason,
        budget_turns: budget.max_turns,
        budget_nodes: budget.max_nodes,
        wall_us: m.wall_us,
        winner,
    }
}

fn emit_csv(rows: &[Row]) -> String {
    let mut out = String::from(
        "scenario,arm,reached_solution,final_tier,final_score,turns,nodes,stop_reason,budget_turns,budget_nodes,wall_us,winner\n",
    );
    for r in rows {
        let _ = writeln!(
            out,
            "{},{},{},{:?},{:.3},{},{},{:?},{},{},{},{}",
            r.scenario,
            r.arm,
            r.reached_solution,
            r.final_tier,
            r.final_score,
            r.turns,
            r.nodes,
            r.stop_reason,
            r.budget_turns,
            r.budget_nodes,
            r.wall_us,
            r.winner,
        );
    }
    out
}

fn print_summary(results: &[ScenarioResult]) {
    println!("\n=== verdict: multiway vs linear at matched budget (pure deterministic stack) ===");
    let mut multiway_wins = 0usize;
    for r in results {
        println!(
            "\n[{}] {}  (budget: {} turns / {} nodes)",
            r.name, r.blurb, r.budget.max_turns, r.budget.max_nodes
        );
        print_arm("linear  ", &r.linear);
        print_arm("multiway", &r.multiway);
        println!("  -> winner: {}", r.winner);
        if r.winner == "multiway" {
            multiway_wins += 1;
        }
    }
    println!(
        "\nmultiway won {}/{} scenarios; the rest tied — reported, not hidden.\n\
         It only pulls ahead with BOTH a local-optimum trap and a budget wide enough to\n\
         explore past it and backtrack: on a smooth landscape greedy reaches the optimum\n\
         with fewer turns, and a starved budget denies the backtrack. turns/nodes are the\n\
         matched-budget evidence; wall_us is informational only (git plumbing dominates).",
        multiway_wins,
        results.len()
    );
}

fn print_arm(name: &str, m: &ArmResult) {
    println!(
        "  {name}: tier={:?} score={:.3} turns={} nodes={} stop={:?} solved={} wall_us={}",
        m.tier, m.score, m.turns, m.nodes, m.stop_reason, m.reached_solution, m.wall_us
    );
}

/// A unique temp directory removed on drop, so a panicking run never leaks a
/// worktree tree or poisons the next run. Mirrors the test harness's `ScratchDir`.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, AtomicOrdering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("nit_mw_bench_{label}_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Initialise `dir` as an operator repo with one commit, so `snapshot` has a HEAD
/// tree to read. The engine commits under its own fixed identity, so this local
/// `user.name`/`email` is never consulted by the store.
fn init_operator_repo(dir: &Path) -> Result<(), Box<dyn Error>> {
    run_git(dir, &["init", "-b", "main"])?;
    run_git(dir, &["config", "user.name", "operator"])?;
    run_git(dir, &["config", "user.email", "operator@example.com"])?;
    std::fs::write(dir.join("seed.txt"), "seed\n")?;
    run_git(dir, &["add", "-A"])?;
    run_git(dir, &["commit", "-m", "initial"])?;
    Ok(())
}

fn run_git(dir: &Path, args: &[&str]) -> Result<(), Box<dyn Error>> {
    let out = Command::new("git").current_dir(dir).args(args).output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("git {args:?} failed: {stderr}").into());
    }
    Ok(())
}

fn git_present() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}
