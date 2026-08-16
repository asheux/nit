//! Shared scaffolding for the Phase 2 acceptance tests: a temp-dir guard, an
//! operator-repo initialiser, and a builder that wires a label-keyed executor and
//! valuer onto a real `GitWorldStore`. Lives in `common/mod.rs` (not a sibling
//! `common.rs`) so cargo does not compile it as its own test binary, and is
//! `allow(dead_code)` because each test binary uses a different subset of it.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use nit_core::GenomeTier;
use nit_multiway::git_store::GitWorldStore;
use nit_multiway::search::Engine;
use nit_multiway::testing::{
    FakeMergeOracle, FakeTurnExecutor, FakeValuer, FileCountValuer, LabelKeyedExecutor,
    ScriptedTurn,
};
use nit_multiway::traits::{ResolvedState, Task};
use nit_multiway::Value;

pub type FakeEngine = Engine<GitWorldStore, LabelKeyedExecutor, FakeValuer, FakeMergeOracle>;

/// The engine shape the Phase 4 merge acceptance tests drive: a real
/// `GitWorldStore` with a scripted [`FakeTurnExecutor`] for the fork turns and a
/// [`FileCountValuer`], whose count-of-`.rs` heuristic lets a merged multi-file
/// tree out-value its single-file parents. [`FakeMergeOracle`] fills the oracle
/// slot for the clean and canned-resolution paths (the engine ignores its returned
/// tree and commits the restored `a` worktree); cases that need a writing or
/// marker-leaving judge build their own engine inline.
pub type MergeEngine = Engine<GitWorldStore, FakeTurnExecutor, FileCountValuer, FakeMergeOracle>;

/// A unique temp directory removed on drop, so a panicking test never leaks a
/// worktree tree or poisons the next run.
pub struct ScratchDir(PathBuf);

impl ScratchDir {
    pub fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("nit_mw_{label}_{}_{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn git_present() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Initialise `dir` as an operator repo with one commit, so `snapshot` has a HEAD
/// tree to read. The engine uses its own fixed identity, so this local config is
/// never consulted by the store.
pub fn init_operator_repo(dir: &Path) {
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("spawn git")
            .status;
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "operator"]);
    git(&["config", "user.email", "operator@example.com"]);
    fs::write(dir.join("seed.txt"), "seed\n").expect("write seed");
    git(&["add", "-A"]);
    git(&["commit", "-m", "initial"]);
}

pub fn viable(score: f32, tier: GenomeTier) -> Value {
    Value {
        gated: false,
        score,
        tier,
    }
}

pub fn gated() -> Value {
    Value {
        gated: true,
        score: 0.0,
        tier: GenomeTier::StillLife,
    }
}

pub fn task() -> Task {
    Task {
        prompt: "multiway-test".to_owned(),
        role: "test".to_owned(),
    }
}

/// Initialise `op` as an operator repo and wire an engine over a real
/// `GitWorldStore`: a [`LabelKeyedExecutor`] scripting `children` and a label-keyed
/// `valuer`. The caller seeds and runs it; `op`/`wt` must outlive the returned
/// engine so its store can clean up.
pub fn build_engine(
    op: &ScratchDir,
    wt: &ScratchDir,
    mission: &str,
    children: &[(&str, &[&str])],
    valuer: FakeValuer,
) -> FakeEngine {
    init_operator_repo(op.path());
    let store = GitWorldStore::new(op.path(), wt.path(), mission).expect("construct store");
    let oracle = FakeMergeOracle::new(ResolvedState {
        tree: op.path().to_path_buf(),
    });
    Engine::new(store, LabelKeyedExecutor::new(children), valuer, oracle)
}

/// Wire a [`MergeEngine`] over a fresh operator repo: the `scripts` are applied to
/// the fork worktrees in call order by the [`FakeTurnExecutor`], and the
/// [`FileCountValuer`] scores every tree by its `.rs` count so a clean join of two
/// disjoint-file branches out-tiers each one-file parent. The caller seeds and
/// runs; `op`/`wt` must outlive the returned engine so its store can clean up.
pub fn build_merge_engine(
    op: &ScratchDir,
    wt: &ScratchDir,
    mission: &str,
    scripts: Vec<ScriptedTurn>,
) -> MergeEngine {
    init_operator_repo(op.path());
    let store = GitWorldStore::new(op.path(), wt.path(), mission).expect("construct store");
    let oracle = FakeMergeOracle::new(ResolvedState {
        tree: op.path().to_path_buf(),
    });
    Engine::new(
        store,
        FakeTurnExecutor::new(scripts),
        FileCountValuer::new(),
        oracle,
    )
}
