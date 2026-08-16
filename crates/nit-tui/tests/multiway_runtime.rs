//! Phase 3 + 4 acceptance — an end-to-end `@multiway` search over a real scratch repo.
//!
//! The runtime spawns a `claude`-backed `RunnerExecutor` off the UI thread, which
//! cannot run deterministically (or offline) in CI. So this drives the exact stack
//! the runtime assembles — real `GitWorldStore` over a temp git repo, the engine's
//! best-first loop, atomic DAG persistence, and store-drop teardown — with the one
//! agent-shaped seam replaced by a scripted backend: `FakeTurnExecutor` for the
//! turns and `FakeValuer` for the genome+gate verdicts (so a "solution" tier is
//! reproducible without a real genome run; the live `GenomeValuer` is covered by
//! `multiway_valuer.rs`). The Phase 3 run uses the `NoMergeOracle` placeholder; the
//! Phase 4 case drives the production `JudgeMergeOracle` via `run_merging`, with the
//! judge turn scripted, so the same offline stack exercises a real adjudicated merge.
//!
//! The load-bearing assertions are the safety contract: the search persists a
//! round-trippable DAG carrying a gated solution node, an adjudicated conflict
//! becomes a viable two-parent node carrying the judge's resolution, the operator's
//! primary tree is byte-for-byte untouched, and dropping the store tears the
//! mission's worktrees and `refs/nit-multiway/<mission>/*` down. A final,
//! runtime-level check proves that with `NIT_MULTIWAY` unset the entry point is inert.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use nit_core::{GenomeTier, MultiwayView};
use nit_multiway::frontier::Frontier;
use nit_multiway::git_store::GitWorldStore;
use nit_multiway::graph::Graph;
use nit_multiway::node::{NodeId, NodeStatus};
use nit_multiway::policy::{Budget, Mood, SearchPolicy};
use nit_multiway::search::{Engine, RunSnapshot, SearchOutcome, StopReason};
use nit_multiway::testing::{FakeTurnExecutor, FakeValuer, LabelKeyedExecutor, ScriptedTurn};
use nit_multiway::traits::{Task, TurnStatus, WorktreeHandle, WorldStore};
use nit_multiway::{Value, Valuer};
use nit_tui::multiway::build_multiway_view;
use nit_tui::multiway::executor::{JudgeMergeOracle, NoMergeOracle};
use nit_tui::multiway::runtime::{
    all_gated_hint, resolve_gate_override, select_valuer, ALL_GATED_HINT,
};
use nit_tui::multiway::GenomeValuer;

struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("nit_mw_rt_{label}_{}_{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git_present() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn git_text(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn git_bytes(dir: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?} failed");
    out.stdout
}

/// Relative path + bytes of every working file except the git dir — so a stray
/// write to even a `.gitignore`d file (the live-editor-buffer threat the engine
/// exists to avoid) would change the fingerprint.
fn working_files(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).expect("read_dir").flatten() {
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            if entry.file_type().expect("file_type").is_dir() {
                pending.push(path);
                continue;
            }
            let relative = path.strip_prefix(root).expect("strip prefix");
            entries.push((
                relative.to_string_lossy().into_owned(),
                fs::read(&path).expect("read file"),
            ));
        }
    }
    entries.sort();
    entries
}

/// Everything about the operator repo the engine must leave intact: HEAD and its
/// symbolic ref (a stray detach or branch move), the porcelain status, the
/// operator-owned ref scopes (`refs/nit-multiway/*` is deliberately excluded — it
/// is the engine's own throwaway namespace), and the working files.
#[derive(Debug, PartialEq)]
struct OperatorState {
    commit: String,
    head_ref: String,
    porcelain: Vec<u8>,
    refs: String,
    files: Vec<(String, Vec<u8>)>,
}

impl OperatorState {
    fn capture(repo: &Path) -> Self {
        Self {
            commit: git_text(repo, &["rev-parse", "HEAD"]),
            head_ref: git_text(repo, &["symbolic-ref", "HEAD"]),
            porcelain: git_bytes(repo, &["status", "--porcelain=v1", "-z"]),
            refs: git_text(
                repo,
                &["for-each-ref", "refs/heads", "refs/tags", "refs/remotes"],
            ),
            files: working_files(repo),
        }
    }
}

fn init_operator_repo(dir: &Path) {
    git_text(dir, &["init", "-b", "main"]);
    git_text(dir, &["config", "user.name", "operator"]);
    git_text(dir, &["config", "user.email", "operator@example.com"]);
    fs::write(dir.join("seed.rs"), "pub fn seed() -> u32 { 0 }\n").expect("write seed");
    // An ignored, untracked file stands in for a live editor buffer: git status
    // ignores it, but `working_files` covers it, so a stray write would be caught.
    fs::write(dir.join(".gitignore"), "scratch.tmp\n").expect("write gitignore");
    fs::write(dir.join("scratch.tmp"), "live-buffer\n").expect("write ignored");
    git_text(dir, &["add", "-A"]);
    git_text(dir, &["commit", "-m", "initial"]);
}

/// The scripted scenario: three speculative edits — a replicator-tier solution, an
/// edit the gate rejects, and a viable runner-up — paired with the label-keyed
/// verdicts the valuer returns. Distinct files and labels give each child a distinct
/// tree (so no two collide on one commit), and the label each turn stamps is the
/// content the valuer keys on, flowing through the same `value(&Path)` seam the real
/// genome valuer uses.
fn scripted_backends() -> (FakeTurnExecutor, FakeValuer) {
    let executor = FakeTurnExecutor::new(vec![
        scripted_edit(
            "solution.rs",
            "pub fn solution() -> u32 { 42 }\n",
            "solution",
        ),
        scripted_edit("broken.rs", "fn broken( {\n", "gate-fail"),
        scripted_edit(
            "runner_up.rs",
            "pub fn runner_up() -> u32 { 7 }\n",
            "runner-up",
        ),
    ]);
    let valuer = FakeValuer::new(Value {
        gated: false,
        score: 0.30,
        tier: GenomeTier::Spaceship,
    })
    .with_label(
        "solution",
        Value {
            gated: false,
            score: 0.95,
            tier: GenomeTier::Replicator,
        },
    )
    .with_label(
        "gate-fail",
        Value {
            gated: true,
            score: 0.50,
            tier: GenomeTier::Spaceship,
        },
    )
    .with_label(
        "runner-up",
        Value {
            gated: false,
            score: 0.40,
            tier: GenomeTier::Methuselah,
        },
    );
    (executor, valuer)
}

fn scripted_edit(file: &str, body: &str, label: &str) -> ScriptedTurn {
    ScriptedTurn {
        status: TurnStatus::Edited,
        summary: format!("scripted edit producing the {label} candidate"),
        writes: vec![(PathBuf::from(file), body.to_owned())],
        label: Some(label.to_owned()),
    }
}

/// The id of the single node in `status`, asserted to be unique.
fn sole_node(graph: &Graph, status: NodeStatus) -> NodeId {
    let mut found = graph
        .iter_nodes()
        .filter(|(_, node)| node.status == status)
        .map(|(id, _)| id.clone());
    let id = found.next().unwrap_or_else(|| panic!("no {status:?} node"));
    assert!(found.next().is_none(), "more than one {status:?} node");
    id
}

#[test]
fn multiway_search_persists_gated_solution_and_leaves_operator_untouched() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }

    let (operator, worktrees_home, pristine) = mission_scratch("op", "wt");
    let op = operator.path();
    let state_home = ScratchDir::new("state");
    let mission = "mis-e2e-001";

    let (executor, valuer) = scripted_backends();
    let store = GitWorldStore::new(op, worktrees_home.path(), mission).expect("construct store");
    let policy = SearchPolicy {
        mood: Mood::Explore,
        k: 3,
        budget: Budget {
            max_nodes: 32,
            max_turns: 16,
            max_tokens: None,
        },
    };
    let task = Task {
        prompt: "improve the scratch crate".to_owned(),
        role: "integrate".to_owned(),
    };

    let mut engine = Engine::new(store, executor, valuer, NoMergeOracle);
    let root = engine.seed(op).expect("seed root");
    let outcome = engine.run(root, &policy, &task).expect("run search");

    // Stopped on the accepted terminal, not the budget or an empty heap. The DAG holds
    // the root plus the three committed children — a gate-failed child stays for
    // provenance; only NoOp/Failed turns commit nothing.
    assert_eq!(outcome.stop_reason, StopReason::Solution);
    assert_eq!(engine.graph.len(), 4);

    let solution_id = sole_node(&engine.graph, NodeStatus::Solution);
    let solution_value = engine
        .graph
        .node(&solution_id)
        .and_then(|node| node.value)
        .expect("the solution node is valued");
    assert!(!solution_value.gated);
    assert!(solution_value.tier >= GenomeTier::Replicator);
    assert_eq!(outcome.best.as_ref(), Some(&solution_id));

    // The gate machinery ran: the rejected edit is in the DAG, marked non-viable, and
    // was never expanded or accepted.
    let gated_id = sole_node(&engine.graph, NodeStatus::GateFailed);
    let gated_value = engine
        .graph
        .node(&gated_id)
        .and_then(|node| node.value)
        .expect("the gate-failed node is valued");
    assert!(gated_value.gated);

    // The root seeded the frontier and is the only parentless node.
    let root_expanded = engine
        .graph
        .iter_nodes()
        .any(|(_, node)| node.parents.is_empty() && node.status == NodeStatus::Expanded);
    assert!(root_expanded, "the root must be expanded");

    // Persist where the runtime would: <state_dir>/multiway/<mission>.json.
    let dag_path = state_home
        .path()
        .join("multiway")
        .join(format!("{mission}.json"));
    engine.graph.save(&dag_path).expect("persist DAG");
    assert!(dag_path.exists(), "the DAG was not written");

    // It round-trips losslessly, solution value and all (the existing graph round-trip
    // covers only `value: None`; this one carries populated scores).
    let reloaded = Graph::load(&dag_path).expect("reload DAG");
    assert_eq!(reloaded.len(), engine.graph.len());
    assert_eq!(
        serde_json::to_string(&engine.graph).expect("encode original"),
        serde_json::to_string(&reloaded).expect("encode reloaded"),
    );
    let reloaded_solution = reloaded
        .node(&solution_id)
        .expect("solution survives reload");
    assert_eq!(reloaded_solution.status, NodeStatus::Solution);
    assert_eq!(reloaded_solution.value, Some(solution_value));

    // Before teardown the engine has anchored its per-turn commits on throwaway refs.
    let refs_pattern = format!("refs/nit-multiway/{mission}");
    let anchored = git_text(op, &["for-each-ref", "--format=%(refname)", &refs_pattern]);
    assert!(
        !anchored.trim().is_empty(),
        "the engine anchored no refs to clean up"
    );

    // Dropping the store is the runtime's teardown path (thread exit / abort / quit all
    // drop it): it removes the worktrees and prunes the mission's refs.
    drop(engine);
    assert_mission_torn_down(worktrees_home.path(), mission, op);

    // The whole point: the operator's primary tree is byte-for-byte unchanged.
    assert_eq!(
        pristine,
        OperatorState::capture(op),
        "the engine mutated the operator's primary tree"
    );
}

/// A no-label scripted edit: these merge turns value trees by the `FakeValuer`
/// default, not by a stamped label, so a single viable verdict covers every tree.
fn unlabeled_edit(file: &str, body: &str) -> ScriptedTurn {
    ScriptedTurn {
        status: TurnStatus::Edited,
        summary: format!("scripted edit to {file}"),
        writes: vec![(PathBuf::from(file), body.to_owned())],
        label: None,
    }
}

const MERGE_RESOLUTION: &str = "pub fn seed() -> u32 { 99 }\n";

#[test]
fn multiway_merge_adjudicates_a_conflict_into_a_viable_two_parent_node() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    let (operator, worktrees_home, pristine) = mission_scratch("merge_op", "merge_wt");
    let op = operator.path();
    let mission = "mis-merge-001";
    let store = GitWorldStore::new(op, worktrees_home.path(), mission).expect("construct store");
    // commit_merge only runs after a real conflict; a git too old for
    // `merge-tree --write-tree` degrades to fork-only — skip rather than fail.
    if !store.merge_supported() {
        eprintln!("skipping: git predates merge-tree --write-tree (2.38)");
        return;
    }

    // The exact stack the runtime assembles for Phase 4 — real `GitWorldStore`, the
    // engine's `run_merging_observed` loop, and the production `JudgeMergeOracle` — with
    // both agent seams scripted: the two forks rewrite `seed.rs` divergently (so git
    // conflicts), and the judge the oracle drives writes the reconciled file into the
    // restored `a` worktree, exactly as a spawned `claude` would.
    let forks = FakeTurnExecutor::new(vec![
        unlabeled_edit("seed.rs", "pub fn seed() -> u32 { 1 }\n"),
        unlabeled_edit("seed.rs", "pub fn seed() -> u32 { 2 }\n"),
    ]);
    let judge = FakeTurnExecutor::new(vec![unlabeled_edit("seed.rs", MERGE_RESOLUTION)]);
    let valuer = FakeValuer::new(Value {
        gated: false,
        score: 0.50,
        tier: GenomeTier::Methuselah,
    });

    let mut engine = Engine::new(store, forks, valuer, JudgeMergeOracle::new(judge));
    let policy = SearchPolicy {
        mood: Mood::Explore,
        k: 2,
        // Two fork turns plus the one judge turn the conflict path charges; the loop
        // stops here, before re-driving the now-exhausted fork script.
        budget: Budget {
            max_nodes: 32,
            max_turns: 3,
            max_tokens: None,
        },
    };
    let task = Task {
        prompt: "reconcile the divergent edits".to_owned(),
        role: "integrate".to_owned(),
    };
    let root = engine.seed(op).expect("seed root");

    // Drive the PRODUCTION observed entry point — `run_merging_observed`, the one
    // `MultiwayRuntime::run_search` installs — building a view per expansion off the
    // live graph/frontier, so this single run proves both the conflict adjudication and
    // that the merge streams into the 6b `MultiwayView` for the live popup.
    let mut views: Vec<MultiwayView> = Vec::new();
    let mut observer = |graph: &Graph, frontier: &Frontier, snap: &RunSnapshot| {
        views.push(build_multiway_view(graph, frontier, snap, &policy, None));
    };
    let outcome = engine
        .run_merging_observed(root, &policy, &task, &mut observer)
        .expect("a conflict is adjudicated, never an abort");

    // One viable two-parent join, and the judge turn was charged to the budget.
    let (merged_id, merged) = engine
        .graph
        .iter_nodes()
        .filter(|(_, node)| node.parents.len() == 2)
        .map(|(id, node)| (id.clone(), node.clone()))
        .next()
        .expect("a two-parent merge node");
    assert!(!merged.value.expect("merge node is valued").gated);
    assert_eq!(merged.status, NodeStatus::Open);
    assert_eq!(outcome.turns_spent, 3);

    // The observer's borrow ended with the run, so `views` is free to read: the live
    // feed streamed one view per expansion and the merge surfaced in the last one as a
    // two-parent row — the "merges render as multi-parent refs" contract a merge-off
    // feed can never show.
    assert_eq!(
        views.len(),
        outcome.nodes_expanded,
        "one streamed view per expand on the production entry point"
    );
    assert!(
        views
            .last()
            .expect("at least one view")
            .nodes
            .iter()
            .any(|node| node.parents.len() == 2),
        "the streamed view carries the two-parent merge node"
    );

    // End-to-end proof: the judge's in-place resolution is the tree the engine
    // committed as the join — restoring the merge node recovers it.
    let restored = engine
        .store
        .restore(&merged_id)
        .expect("restore merged tree");
    assert_eq!(
        fs::read_to_string(restored.path().join("seed.rs")).expect("merged seed.rs"),
        MERGE_RESOLUTION,
    );
    drop(restored);

    // The Phase 4 merge path obeys the same safety contract: store-drop tears the
    // worktrees down, prunes the mission refs, and the operator's primary tree is
    // byte-for-byte untouched.
    drop(engine);
    assert_mission_torn_down(worktrees_home.path(), mission, op);
    assert_eq!(
        pristine,
        OperatorState::capture(op),
        "the merge mutated the operator's primary tree"
    );
}

#[test]
fn flag_off_entry_path_is_inert() {
    use std::sync::Mutex;

    use nit_tui::multiway::MultiwayRuntime;

    // `from_env` reads `NIT_MULTIWAY` once; serialize the mutation so a sibling test
    // never observes a half-set var.
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let prior = std::env::var("NIT_MULTIWAY").ok();

    // Unset: the runtime is inert — `@multiway …` falls through to ordinary chat
    // dispatch and no search machinery is constructed (the byte-identical off-path).
    std::env::remove_var("NIT_MULTIWAY");
    assert!(!MultiwayRuntime::from_env().enabled());

    // Set: the operator opted in, so the entry point is live.
    std::env::set_var("NIT_MULTIWAY", "1");
    assert!(MultiwayRuntime::from_env().enabled());

    // Explicit `0` is off, same as unset.
    std::env::set_var("NIT_MULTIWAY", "0");
    assert!(!MultiwayRuntime::from_env().enabled());

    match prior {
        Some(value) => std::env::set_var("NIT_MULTIWAY", value),
        None => std::env::remove_var("NIT_MULTIWAY"),
    }
}

#[test]
fn runtime_start_then_abort_tears_down_without_leaking() {
    use std::thread;
    use std::time::Duration;

    use nit_tui::claude_runner::{ClaudeRunner, ClaudeRunnerConfig};
    use nit_tui::multiway::MultiwayRuntime;

    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }

    let operator = ScratchDir::new("rt_abort_op");
    let op = operator.path();
    init_operator_repo(op);
    let pristine = OperatorState::capture(op);

    // Drive the runtime seam, not the engine stack: `start` spawns the search
    // thread that owns the `GitWorldStore`, and `abort` flips the cancel flag the
    // executor checks at the top of its first `run_turn` — so the turn returns
    // before spawning a real `claude`, keeping this deterministic and offline.
    // `start` ignores the env flag (that only gates the caller), so the seam runs
    // regardless of the ambient `NIT_MULTIWAY`.
    let mission = format!("mis-rt-abort-{}", std::process::id());
    let runner = ClaudeRunner::spawn(ClaudeRunnerConfig::default());
    let mut runtime = MultiwayRuntime::from_env();
    runtime
        .start(
            &mission,
            op.to_path_buf(),
            runner,
            "claude".to_owned(),
            Task {
                prompt: "aborted before any turn runs".to_owned(),
                role: "integrate".to_owned(),
            },
            SearchPolicy {
                mood: Mood::Balanced,
                k: 2,
                budget: Budget {
                    max_nodes: 8,
                    max_turns: 4,
                    max_tokens: None,
                },
            },
        )
        .expect("start the multiway search");
    runtime.abort(&mission);

    // Wait for the thread to unwind, drop its store (cleanup), and signal
    // `finished`; drain the terminal event it emits on the way out.
    let mut terminal_seen = false;
    let mut waited = Duration::ZERO;
    let step = Duration::from_millis(20);
    while waited < Duration::from_secs(20) {
        terminal_seen |= drain_terminal(runtime.poll(&mission));
        if !runtime.is_active(&mission) {
            break;
        }
        thread::sleep(step);
        waited += step;
    }
    terminal_seen |= drain_terminal(runtime.poll(&mission));
    runtime.cleanup_finished();

    assert!(
        !runtime.is_active(&mission),
        "the aborted search is still marked active"
    );
    assert!(
        terminal_seen,
        "the aborted search emitted no terminal event"
    );

    // The start->abort->cleanup seam tore everything down: the mission's
    // worktrees dir and its `refs/nit-multiway/<mission>/*` are gone, and the
    // operator's primary tree is byte-for-byte untouched.
    if let Some(base) = nit_utils::paths::state_dir().or_else(nit_utils::paths::data_dir) {
        let worktrees = base.join("multiway").join("worktrees").join(&mission);
        let leftover = fs::read_dir(&worktrees)
            .map(|mut dir| dir.next().is_some())
            .unwrap_or(false);
        assert!(
            !leftover,
            "worktrees survived teardown at {}",
            worktrees.display()
        );
    }
    let refs = git_text(
        op,
        &[
            "for-each-ref",
            "--format=%(refname)",
            &format!("refs/nit-multiway/{mission}"),
        ],
    );
    assert!(refs.trim().is_empty(), "mission refs leaked: {refs}");
    assert_eq!(
        pristine,
        OperatorState::capture(op),
        "the runtime mutated the operator's primary tree"
    );
}

/// True when any event in the batch is a terminal `Done`/`Failed`.
fn drain_terminal(events: Vec<nit_tui::multiway::MultiwayEvent>) -> bool {
    events.into_iter().any(|event| {
        matches!(
            event,
            nit_tui::multiway::MultiwayEvent::Done(_) | nit_tui::multiway::MultiwayEvent::Failed(_)
        )
    })
}

/// Stand up an operator repo and its worktrees home for a search-path test: returns
/// the two scratch dirs (the caller keeps them alive for RAII cleanup) and the
/// operator's pristine fingerprint to compare against after teardown.
fn mission_scratch(
    op_label: &str,
    worktrees_label: &str,
) -> (ScratchDir, ScratchDir, OperatorState) {
    let operator = ScratchDir::new(op_label);
    init_operator_repo(operator.path());
    let pristine = OperatorState::capture(operator.path());
    let worktrees_home = ScratchDir::new(worktrees_label);
    (operator, worktrees_home, pristine)
}

/// The store-drop teardown contract every search path shares: once the engine (and
/// its `GitWorldStore`) has dropped, the mission's worktrees are gone and its
/// `refs/nit-multiway/<mission>/*` are pruned. The operator-pristine compare stays at
/// the call site, where each test frames that failure in its own words.
fn assert_mission_torn_down(worktrees_home: &Path, mission: &str, op: &Path) {
    let mission_root = worktrees_home.join(mission);
    let leftover = fs::read_dir(&mission_root)
        .map(|mut dir| dir.next().is_some())
        .unwrap_or(false);
    assert!(!leftover, "worktrees survived teardown");
    let pruned = git_text(
        op,
        &[
            "for-each-ref",
            "--format=%(refname)",
            &format!("refs/nit-multiway/{mission}"),
        ],
    );
    assert!(pruned.trim().is_empty(), "refs survived teardown: {pruned}");
}

/// Phase 6b: `run_observed` fires its observer once per expansion, each fire can
/// build a `MultiwayView` snapshot (the live feed `drive_multiway` drains into the
/// popup), and observing leaves the store-drop teardown the search relies on
/// untouched. This drives the engine seam directly — the runtime's `run_search`
/// installs the same observer over a spawned `claude`, which can't run offline.
#[test]
fn run_observed_streams_a_view_per_expansion_and_cleans_up() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }

    let (operator, worktrees_home, pristine) = mission_scratch("observed_op", "observed_wt");
    let op = operator.path();
    let mission = "mis-observed-001";
    let store = GitWorldStore::new(op, worktrees_home.path(), mission).expect("construct store");

    // A label-keyed tree with no Replicator-tier node never early-stops on a
    // solution, so the search expands until the node cap — several expansions, so
    // "one view per expand" is a plural claim, not a 1:1 coincidence.
    let executor = LabelKeyedExecutor::new(&[
        ("", &["hi", "lo"]),
        ("hi", &["hi", "lo"]),
        ("lo", &["hi", "lo"]),
    ]);
    let valuer = FakeValuer::new(Value {
        gated: false,
        score: 0.30,
        tier: GenomeTier::Spaceship,
    })
    .with_label(
        "hi",
        Value {
            gated: false,
            score: 0.80,
            tier: GenomeTier::Methuselah,
        },
    )
    .with_label(
        "lo",
        Value {
            gated: false,
            score: 0.30,
            tier: GenomeTier::Spaceship,
        },
    );
    let policy = SearchPolicy {
        mood: Mood::Explore,
        k: 2,
        budget: Budget {
            max_nodes: 7,
            max_turns: 100,
            max_tokens: None,
        },
    };
    let task = Task {
        prompt: "stream the search".to_owned(),
        role: "integrate".to_owned(),
    };

    let mut engine = Engine::new(store, executor, valuer, NoMergeOracle);
    let root = engine.seed(op).expect("seed root");

    // The observer the runtime installs: build a view per expansion. It captures
    // only the view sink and the policy — never the engine, store, or a worktree.
    let mut views: Vec<MultiwayView> = Vec::new();
    let mut observer = |graph: &Graph, frontier: &Frontier, snap: &RunSnapshot| {
        views.push(build_multiway_view(graph, frontier, snap, &policy, None));
    };
    let outcome = engine
        .run_observed(root, &policy, &task, &mut observer)
        .expect("observed search runs");

    // The observer's borrow of `views`/`policy` ends here (last use), so both are
    // free to read below. One streamed view per expansion, and more than one
    // expansion happened.
    assert!(
        outcome.nodes_expanded >= 2,
        "expected a multi-step search, got {}",
        outcome.nodes_expanded
    );
    assert_eq!(
        views.len(),
        outcome.nodes_expanded,
        "one streamed view per expand"
    );

    // The final view mirrors the whole DAG and carries the header knobs + lineage.
    let final_view = views.last().expect("at least one view");
    assert_eq!(final_view.nodes.len(), engine.graph.len());
    assert_eq!(final_view.header.k, policy.k);
    assert!(
        !final_view.kept_path.is_empty(),
        "a best lineage is highlighted"
    );

    // Teardown discipline is intact: dropping the engine drops the store, which
    // removes the worktrees and prunes the mission refs; the operator tree is
    // byte-for-byte untouched throughout.
    drop(engine);
    assert_mission_torn_down(worktrees_home.path(), mission, op);
    assert_eq!(
        pristine,
        OperatorState::capture(op),
        "the observed search mutated the operator tree"
    );
}

/// Phase 8.1: `NIT_MULTIWAY_GATES` resolves to a tri-state ONCE at construction.
/// The empty string is the documented fail-open — `Some(vec![])`, genome-only —
/// and must stay distinct from `None` (unset, auto-detect); a lone separator or
/// whitespace collapses to the same empty set, never back to `None`.
#[test]
fn gate_override_resolves_to_a_tristate() {
    assert_eq!(resolve_gate_override(None), None);
    assert_eq!(resolve_gate_override(Some("")), Some(Vec::new()));
    assert_eq!(resolve_gate_override(Some(";")), Some(Vec::new()));
    assert_eq!(resolve_gate_override(Some("   ")), Some(Vec::new()));

    // One command keeps its inner whitespace — the argv split happens later, at
    // spawn time, with no shell.
    assert_eq!(
        resolve_gate_override(Some("uv run pytest -q")),
        Some(vec!["uv run pytest -q".to_owned()])
    );
    // `;`-separated commands split and trim; an empty middle entry drops out.
    assert_eq!(
        resolve_gate_override(Some("cargo test ;; cargo clippy ")),
        Some(vec!["cargo test".to_owned(), "cargo clippy".to_owned()])
    );
}

/// The resolved override drives valuer selection: a non-empty set runs those
/// commands (a failing one gates, a passing one does not), the empty set is
/// genome-only, and `None` routes to `for_tree` verbatim. Uses the always-present
/// `true`/`false` utilities so the gate verdict is deterministic and offline.
#[test]
fn select_valuer_honors_the_gate_override_modes() {
    let tree = ScratchDir::new("valuer_modes");
    fs::write(tree.path().join("clean.rs"), "pub fn ok() -> u32 { 1 }\n").expect("write source");
    // Bound `for_tree`'s ancestor walk to this tree so a manifest above an ambient
    // `TMPDIR` can't leak a real bundle into the unset case; `source_files` skips
    // the dot-dir, so it never enters valuation.
    fs::create_dir(tree.path().join(".git")).expect("mark repo root");

    let value = |gate_override| {
        select_valuer(gate_override, tree.path())
            .value(tree.path())
            .expect("valuation runs")
    };

    // SET: the commands run — a failing gate marks the node gated...
    assert!(
        value(Some(vec!["false".to_owned()])).gated,
        "a failing gate command must gate the node"
    );
    // ...and a passing gate leaves a clean tree viable.
    assert!(
        !value(Some(vec!["true".to_owned()])).gated,
        "a passing gate must not gate a clean tree"
    );
    // EMPTY: genome-only — no command runs, so a clean tree is never gated.
    assert!(!value(Some(Vec::new())).gated);

    // UNSET: routes to `for_tree`, byte-identical to a direct call (a bare tree
    // detects no bundle, so the fallback is itself genome-only).
    let unset = value(None);
    let for_tree = GenomeValuer::for_tree(tree.path())
        .value(tree.path())
        .expect("valuation runs");
    assert_eq!(unset, for_tree, "unset must route through for_tree");
    assert!(!unset.gated);
}

/// The all-gated steer fires exactly when a finished search kept no viable node
/// (`best` is `None`) and names the override, the clean-worktree example, and the
/// genome-only escape — so the operator can act on it without reading the docs.
#[test]
fn all_gated_hint_fires_only_when_no_node_survives() {
    let none_survived = SearchOutcome {
        best: None,
        nodes_expanded: 3,
        turns_spent: 3,
        stop_reason: StopReason::FrontierEmpty,
    };
    let hint = all_gated_hint(&none_survived).expect("an all-gated finish surfaces the steer");
    assert_eq!(hint, ALL_GATED_HINT);
    assert!(hint.contains("all candidates gate-failed"));
    assert!(hint.contains("NIT_MULTIWAY_GATES"));
    assert!(hint.contains("uv run pytest -q"));
    assert!(hint.contains("NIT_MULTIWAY_GATES= for genome-only"));

    let solved = SearchOutcome {
        best: Some(NodeId::new("node-1")),
        nodes_expanded: 4,
        turns_spent: 4,
        stop_reason: StopReason::Solution,
    };
    assert_eq!(all_gated_hint(&solved), None);
}

/// End-to-end: when the valuer gates every speculative edit, no child commits as
/// viable, the frontier empties with no best node, and `all_gated_hint` therefore
/// surfaces the steer — proving the `best.is_none()` keying is real, not assumed.
#[test]
fn an_all_gated_search_yields_no_best_and_surfaces_the_steer() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }

    let (operator, worktrees_home, pristine) = mission_scratch("all_gated_op", "all_gated_wt");
    let op = operator.path();
    let mission = "mis-all-gated-001";
    let store = GitWorldStore::new(op, worktrees_home.path(), mission).expect("construct store");

    // Unlabeled edits take the valuer's default verdict — gated — so every child
    // fails its gate; distinct files keep the two trees from colliding on one commit.
    let executor = FakeTurnExecutor::new(vec![
        unlabeled_edit("a.rs", "pub fn a() -> u32 { 1 }\n"),
        unlabeled_edit("b.rs", "pub fn b() -> u32 { 2 }\n"),
    ]);
    let valuer = FakeValuer::new(Value {
        gated: true,
        score: 0.20,
        tier: GenomeTier::Spaceship,
    });
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
        prompt: "every candidate fails its gate".to_owned(),
        role: "integrate".to_owned(),
    };

    let mut engine = Engine::new(store, executor, valuer, NoMergeOracle);
    let root = engine.seed(op).expect("seed root");
    let outcome = engine.run(root, &policy, &task).expect("run search");

    assert_eq!(
        outcome.best, None,
        "a gate-failed child must never become best"
    );
    assert_eq!(outcome.stop_reason, StopReason::FrontierEmpty);
    assert_eq!(all_gated_hint(&outcome), Some(ALL_GATED_HINT));

    drop(engine);
    assert_mission_torn_down(worktrees_home.path(), mission, op);
    assert_eq!(
        pristine,
        OperatorState::capture(op),
        "the all-gated search mutated the operator tree"
    );
}
