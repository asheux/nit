//! Phase 2 acceptance — the deterministic trap.
//!
//! Greedy (`Exploit`) and multiway (`Explore`) run the SAME fixture at the SAME
//! budget; only the mood differs. Greedy admits one child per expansion, so it
//! commits to the locally-best branch, holds the globally-correct one off the
//! frontier, and spends its whole budget stuck at a Methuselah local optimum.
//! Multiway admits every viable child, so once the greedy branch's children
//! regress below it, `Frontier::pop_best` re-selects the held-back branch —
//! emergent backtracking — and reaches the Replicator global optimum. The asserted
//! tier/score/stop-reason gap fails if backtracking ever regresses to greedy
//! behaviour, and the single shared `Budget` makes a rigged, unequal budget
//! impossible to slip in.
//!
//! A real `GitWorldStore` (temp repo) is used on purpose: git SHAs mix parent and
//! content, so the deep plateau's equal-value nodes never alias to one `NodeId`.

mod common;

use nit_core::GenomeTier;
use nit_multiway::policy::{Budget, Mood, SearchPolicy};
use nit_multiway::search::{SearchOutcome, StopReason};
use nit_multiway::testing::FakeValuer;
use nit_multiway::Value;

use common::{build_engine, git_present, task, viable, ScratchDir};

/// Run the trap fixture under `mood` at `budget`, returning the outcome and the
/// `Value` of the best node the search settled on.
fn run_trap(mood: Mood, budget: Budget) -> (SearchOutcome, Value) {
    let op = ScratchDir::new("trap_op");
    let wt = ScratchDir::new("trap_wt");

    // Root forks into `trunk` (the tempting line) and a low `side`. `trunk` forks
    // into `hi` — a Methuselah local optimum whose children regress to a 0.30
    // plateau — and `lo`, which scores lower now but is the only path to the
    // `lo_win` Replicator. The plateau re-stamps itself, so greedy never runs out
    // of locally-best moves to make.
    let children: &[(&str, &[&str])] = &[
        ("", &["trunk", "side"]),
        ("trunk", &["hi", "lo"]),
        ("hi", &["hi_low", "hi_low"]),
        ("hi_low", &["hi_low", "hi_low"]),
        ("lo", &["lo_win", "lo_meh"]),
    ];
    let valuer = FakeValuer::new(viable(0.0, GenomeTier::StillLife))
        .with_label("trunk", viable(0.70, GenomeTier::Spaceship))
        .with_label("side", viable(0.10, GenomeTier::StillLife))
        .with_label("hi", viable(0.80, GenomeTier::Methuselah))
        .with_label("lo", viable(0.40, GenomeTier::Spaceship))
        .with_label("hi_low", viable(0.30, GenomeTier::Spaceship))
        .with_label("lo_win", viable(0.95, GenomeTier::Replicator))
        .with_label("lo_meh", viable(0.30, GenomeTier::Spaceship));

    let mut engine = build_engine(&op, &wt, "trap", children, valuer);
    let policy = SearchPolicy { mood, k: 2, budget };

    let root = engine.seed(op.path()).expect("seed root");
    let outcome = engine.run(root, &policy, &task()).expect("run search");
    let best = outcome.best.clone().expect("a viable best node");
    let value = engine
        .graph
        .node(&best)
        .expect("best node present")
        .value
        .expect("best node is valued");
    (outcome, value)
}

#[test]
fn greedy_traps_locally_while_multiway_reaches_the_global_solution() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }

    // One budget, handed verbatim to both runs — `Budget` is `Copy`, so "equal
    // budget" is a property of the code, not a claim to be eyeballed.
    let budget = Budget {
        max_nodes: 64,
        max_turns: 12,
        max_tokens: None,
    };
    let (exploit, exploit_best) = run_trap(Mood::Exploit, budget);
    let (explore, explore_best) = run_trap(Mood::Explore, budget);

    // Greedy spends every turn and is still stuck at the local optimum: it admitted
    // only the locally-best child each step, so `lo` stayed off the frontier and the
    // Replicator branch beyond it was never reached.
    assert_eq!(exploit.stop_reason, StopReason::BudgetExhausted);
    assert_eq!(exploit_best.tier, GenomeTier::Methuselah);
    assert!(
        exploit.turns_spent >= budget.max_turns,
        "greedy must exhaust the budget, not stop early ({} turns)",
        exploit.turns_spent
    );

    // Multiway, at the identical budget, kept `lo` alive, backtracked to it once
    // `hi`'s children regressed, and accepted the Replicator global optimum with
    // budget to spare.
    assert_eq!(explore.stop_reason, StopReason::Solution);
    assert_eq!(explore_best.tier, GenomeTier::Replicator);
    assert!(
        explore.turns_spent < budget.max_turns,
        "multiway must solve within the same budget greedy exhausted ({} turns)",
        explore.turns_spent
    );

    // The gap, asserted exactly: any regression that collapses Explore into Exploit
    // (or the reverse) breaks one of these.
    assert!(
        explore_best.tier > exploit_best.tier,
        "multiway must out-tier greedy: {:?} !> {:?}",
        explore_best.tier,
        exploit_best.tier
    );
    assert!(
        explore_best.score > exploit_best.score,
        "multiway must out-score greedy: {} !> {}",
        explore_best.score,
        exploit_best.score
    );
}
