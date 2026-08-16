//! Phase 1 acceptance for the real genome+gates `Valuer` (`nit_tui::multiway`):
//! a failing gate marks a worktree non-viable (`gated`) while its genome score
//! is still measured, a passing gate leaves it viable, and a better-structured
//! tree scores strictly higher. Gates are injected as deterministic `true` /
//! `false` commands so the test is not a flaky multi-second `cargo` spawn.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use nit_multiway::git_store::GitWorldStore;
use nit_multiway::node::NodeStatus;
use nit_multiway::policy::{Budget, Mood, SearchPolicy};
use nit_multiway::search::{Engine, StopReason};
use nit_multiway::testing::{FakeMergeOracle, FakeTurnExecutor, ScriptedTurn};
use nit_multiway::traits::{ResolvedState, Task, TurnStatus};
use nit_multiway::Valuer;
use nit_tui::multiway::GenomeValuer;

/// A program guaranteed absent from `PATH`, so spawning it ENOENTs. Before Phase
/// 8.2 the spawn error surfaced as `Err` and aborted the entire search on its
/// first valuation; it must now gate the node and let the search finish.
const MISSING_GATE: &str = "nit-multiway-no-such-gate-program";

/// Idiomatic enum + `match` state machine — scores Spaceship.
const BETTER: &str = r#"
pub enum State {
    Idle,
    Running,
    Paused,
    Stopped,
}

pub struct Machine {
    state: State,
    ticks: u32,
}

impl Machine {
    pub fn new() -> Self {
        Self { state: State::Idle, ticks: 0 }
    }

    pub fn step(&mut self) {
        self.ticks += 1;
        self.state = match self.state {
            State::Idle => State::Running,
            State::Running if self.ticks > 5 => State::Paused,
            State::Running => State::Running,
            State::Paused => State::Stopped,
            State::Stopped => State::Stopped,
        };
    }

    pub fn label(&self) -> &'static str {
        match self.state {
            State::Idle => "idle",
            State::Running => "running",
            State::Paused => "paused",
            State::Stopped => "stopped",
        }
    }
}
"#;

/// A worktree still holding both Git conflict bookends — the unresolved (dead) merge
/// the Phase-4 valuer must force-gate regardless of its genome score.
const CONFLICTED: &str = r#"
pub fn tally(items: &[u32]) -> u32 {
    let mut total = 0;
    for item in items {
        total += item;
    }
<<<<<<< ours
    total.saturating_mul(2)
=======
    total.saturating_add(items.len() as u32)
>>>>>>> theirs
}
"#;

/// The same code with the conflict resolved — the viable control twin: identical
/// structure, markers removed, so only the bookends distinguish the two verdicts.
const RESOLVED: &str = r#"
pub fn tally(items: &[u32]) -> u32 {
    let mut total = 0;
    for item in items {
        total += item;
    }
    total.saturating_mul(2)
}
"#;

/// One function buried under a deeply nested if/else pyramid — scores Still Life.
const WORSE: &str = r#"
fn process(a: i64, b: i64, c: i64, d: i64) -> i64 {
    let mut x = 0;
    if a > 0 {
        if b > 0 {
            if c > 0 {
                if d > 0 {
                    x = a + b + c + d;
                } else {
                    x = a + b + c;
                }
            } else {
                if d > 0 {
                    x = a + b + d;
                } else {
                    x = a + b;
                }
            }
        } else {
            if c > 0 {
                x = a + c;
            } else {
                x = a;
            }
        }
    } else {
        x = 0;
    }
    x
}
"#;

#[test]
fn failing_gate_marks_tree_gated_but_still_scored() {
    let tree = ScratchTree::new("failing-gate", BETTER);
    let value = GenomeValuer::new(vec!["false".to_string()])
        .value(tree.path())
        .expect("valuation succeeds");

    assert!(value.gated, "a non-zero gate exit must gate the node out");
    assert!(
        value.score > 0.0,
        "the genome score is measured even when gated"
    );
}

#[test]
fn passing_gate_leaves_tree_viable() {
    let tree = ScratchTree::new("passing-gate", BETTER);
    let value = GenomeValuer::new(vec!["true".to_string()])
        .value(tree.path())
        .expect("valuation succeeds");

    assert!(!value.gated);
    assert!(value.score > 0.0);
}

#[test]
fn better_tree_scores_strictly_higher() {
    let better_tree = ScratchTree::new("better", BETTER);
    let worse_tree = ScratchTree::new("worse", WORSE);
    let pass = || GenomeValuer::new(vec!["true".to_string()]);

    let better = pass()
        .value(better_tree.path())
        .expect("valuation succeeds");
    let worse = pass().value(worse_tree.path()).expect("valuation succeeds");

    assert!(!better.gated && !worse.gated);
    assert!(
        better.score > worse.score,
        "better tree must score higher: {} vs {}",
        better.score,
        worse.score
    );
}

#[test]
fn unresolved_conflict_markers_force_gate_the_tree() {
    // No gate command is supplied, so `gated` can be set only by the conflict-marker
    // check: a tree still carrying both bookends is an unresolved merge the valuer
    // withholds from the frontier even when the language has no build gate bundle —
    // the Phase-4 safety net so a botched judge adjudication never passes as viable.
    let conflicted = ScratchTree::new("conflicted", CONFLICTED);
    let resolved = ScratchTree::new("resolved", RESOLVED);
    let ungated = || GenomeValuer::new(Vec::new());

    let dead = ungated()
        .value(conflicted.path())
        .expect("valuation succeeds");
    assert!(
        dead.gated,
        "a tree holding <<<<<<< / >>>>>>> bookends must be force-gated"
    );

    // The resolved twin proves the markers — not the content or a gate — were the
    // gate, and its genome is still measured.
    let clean = ungated()
        .value(resolved.path())
        .expect("valuation succeeds");
    assert!(!clean.gated, "the resolved twin is viable");
    assert!(clean.score > 0.0, "the genome score is still measured");
}

#[test]
fn missing_gate_program_gates_node_instead_of_aborting() {
    let tree = ScratchTree::new("enoent-gate", BETTER);
    let value = GenomeValuer::new(vec![MISSING_GATE.to_string()])
        .value(tree.path())
        .expect("a gate program that cannot be spawned must not abort valuation");

    assert!(
        value.gated,
        "a gate whose program cannot be spawned gates the node out"
    );
    assert!(
        value.score > 0.0,
        "the genome is still measured even when the gate program is missing"
    );
}

#[test]
fn missing_gate_program_lets_the_search_run_to_completion() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    // A real engine over a real GitWorldStore valued by the genuine GenomeValuer,
    // whose only gate is a program that ENOENTs. Each forked child must gate rather
    // than abort, so the frontier drains and the run stops cleanly — the exact
    // robustness Phase 8.2 buys over the pre-fix `Err`-and-abort-on-first-valuation.
    let op = ScratchTree::new("enoent-op", BETTER);
    init_git_repo(op.path());
    let worktrees = ScratchTree::new("enoent-wt", BETTER);

    let store =
        GitWorldStore::new(op.path(), worktrees.path(), "enoent").expect("construct git store");
    let oracle = FakeMergeOracle::new(ResolvedState {
        tree: op.path().to_path_buf(),
    });
    let scripts = vec![scripted_edit("child_a.rs"), scripted_edit("child_b.rs")];
    let valuer = GenomeValuer::new(vec![MISSING_GATE.to_string()]);
    let mut engine = Engine::new(store, FakeTurnExecutor::new(scripts), valuer, oracle);

    let policy = SearchPolicy {
        mood: Mood::Explore,
        k: 2,
        budget: Budget {
            max_nodes: 16,
            max_turns: 8,
            max_tokens: None,
        },
    };
    let task = Task {
        prompt: "enoent gate".to_string(),
        role: "test".to_string(),
    };
    let root = engine.seed(op.path()).expect("seed root");
    let outcome = engine
        .run(root, &policy, &task)
        .expect("a missing gate program must not abort the search");

    assert_eq!(
        outcome.stop_reason,
        StopReason::FrontierEmpty,
        "every child gated, so the frontier drains and the search stops cleanly"
    );
    assert!(
        outcome.best.is_none(),
        "no child is viable when the gate program is missing"
    );
    let gate_failed = engine
        .graph
        .iter_nodes()
        .filter(|(_, node)| node.status == NodeStatus::GateFailed)
        .count();
    assert_eq!(
        gate_failed, 2,
        "both forked children are committed for provenance and recorded GateFailed"
    );
}

/// One scripted fork turn writing `file` (the BETTER fixture) into the worktree,
/// so each child carries real genome content and the missing-program gate is what
/// decides its verdict.
fn scripted_edit(file: &str) -> ScriptedTurn {
    ScriptedTurn {
        status: TurnStatus::Edited,
        summary: format!("add {file}"),
        writes: vec![(PathBuf::from(file), BETTER.to_string())],
        label: None,
    }
}

fn git_present() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Turn an existing scratch dir into a one-commit operator repo so the store's
/// `snapshot` has a HEAD tree to read. A fixed identity keeps the commit
/// independent of the host's git config.
fn init_git_repo(dir: &Path) {
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("spawn git")
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "nit-test"]);
    git(&["config", "user.email", "nit-test@example.com"]);
    git(&["add", "-A"]);
    git(&["commit", "-m", "seed"]);
}

/// A throwaway worktree under the temp dir holding a single `.rs` fixture,
/// removed on drop. No `tempfile` dep (forbidden) — a unique per-test name keeps
/// concurrent test threads isolated.
struct ScratchTree {
    path: PathBuf,
}

impl ScratchTree {
    fn new(label: &str, fixture: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!("nit-mw-valuer-{}-{label}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create scratch tree");
        fs::write(path.join("candidate.rs"), fixture).expect("write fixture");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
