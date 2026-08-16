//! In-memory test doubles for the engine traits — a public module (not
//! `#[cfg(test)]`) so integration tests in `tests/` can build engines without a
//! git repo or a spawned agent. The executor applies scripted file writes and
//! optionally stamps a label into [`FAKE_LABEL_FILE`]; the valuer reads that
//! label to return a scripted [`Value`], so per-child values follow tree content
//! through the `value(&Path)` seam, exactly as the real valuer does.
//!
//! For the Phase 4 merge tests, [`FileCountValuer`] scores a tree by its
//! source-file count (so a join out-values its parents) and [`ScriptedMergeOracle`]
//! resolves a conflict by writing into the `a` worktree, as the production judge does.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use nit_core::GenomeTier;

use crate::traits::{
    Conflicts, MergeOracle, ResolvedState, Task, TurnExecutor, TurnResult, TurnStatus, Valuer,
};
use crate::value::Value;

pub const FAKE_LABEL_FILE: &str = ".nit-multiway-fake-label";

/// File the [`LabelKeyedExecutor`] rewrites every turn so two same-label sibling
/// commits never hash to one git SHA: distinct content keeps them distinct
/// `NodeId`s even when their scripted genome value is identical.
pub const FAKE_SEQ_FILE: &str = ".nit-multiway-fake-seq";

#[derive(Debug, thiserror::Error)]
pub enum FakeError {
    #[error("fake executor: no scripted turn for index {0}")]
    ScriptExhausted(usize),
    #[error("fake executor: writing the scripted tree failed")]
    Io(#[from] std::io::Error),
    #[error("fake valuer: no scripted value for label `{0}`")]
    UnknownLabel(String),
    #[error("fake executor: no scripted expansion for parent label `{0}`")]
    NoExpansion(String),
}

/// One scripted turn: file writes relative to the worktree, plus an optional
/// label stamped into [`FAKE_LABEL_FILE`] for the valuer to key its `Value` on.
#[derive(Clone, Debug)]
pub struct ScriptedTurn {
    pub status: TurnStatus,
    pub summary: String,
    pub writes: Vec<(PathBuf, String)>,
    pub label: Option<String>,
}

pub struct FakeTurnExecutor {
    scripts: Vec<ScriptedTurn>,
    cursor: AtomicUsize,
}

impl FakeTurnExecutor {
    pub fn new(scripts: Vec<ScriptedTurn>) -> Self {
        Self {
            scripts,
            cursor: AtomicUsize::new(0),
        }
    }
}

impl TurnExecutor for FakeTurnExecutor {
    type Error = FakeError;

    fn run_turn(&self, worktree: &Path, _task: &Task) -> Result<TurnResult, Self::Error> {
        let idx = self.cursor.fetch_add(1, Ordering::SeqCst);
        let script = self
            .scripts
            .get(idx)
            .ok_or(FakeError::ScriptExhausted(idx))?;

        let mut changed = Vec::with_capacity(script.writes.len());
        for (rel, contents) in &script.writes {
            let target = worktree.join(rel);
            if let Some(dir) = target.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&target, contents)?;
            changed.push(rel.clone());
        }
        if let Some(label) = &script.label {
            std::fs::write(worktree.join(FAKE_LABEL_FILE), label)?;
        }

        Ok(TurnResult {
            status: script.status,
            summary: script.summary.clone(),
            changed_paths: changed,
        })
    }
}

/// A [`TurnExecutor`] that scripts a whole search tree by the label a node
/// inherits from its parent, rather than by a single global call cursor. The
/// engine forks a parent into `k` worktrees that all carry the parent's
/// [`FAKE_LABEL_FILE`]; this reads that label, stamps the parent's next child
/// label, and bumps [`FAKE_SEQ_FILE`]. Selecting the script by *content* makes a
/// multi-round expansion deterministic no matter what order the frontier draws
/// parents in — the determinism the trap test depends on, which a global cursor
/// (see [`FakeTurnExecutor`]) cannot give once backtracking reorders expansions.
pub struct LabelKeyedExecutor {
    children: HashMap<String, Vec<String>>,
    cursors: Mutex<HashMap<String, usize>>,
    seq: AtomicUsize,
}

impl LabelKeyedExecutor {
    /// Each entry maps a parent label — the empty string for the unlabeled seed
    /// root — to the child labels its fork stamps, one per slot. The forks of one
    /// parent cycle through the slots in order, so the same `k` children are
    /// produced every time that label is expanded.
    pub fn new(children: &[(&str, &[&str])]) -> Self {
        let children = children
            .iter()
            .map(|(parent, kids)| {
                let kids = kids.iter().map(|k| (*k).to_owned()).collect();
                ((*parent).to_owned(), kids)
            })
            .collect();
        Self {
            children,
            cursors: Mutex::new(HashMap::new()),
            seq: AtomicUsize::new(0),
        }
    }
}

impl TurnExecutor for LabelKeyedExecutor {
    type Error = FakeError;

    fn run_turn(&self, worktree: &Path, _task: &Task) -> Result<TurnResult, Self::Error> {
        let parent = std::fs::read_to_string(worktree.join(FAKE_LABEL_FILE))
            .map(|raw| raw.trim().to_owned())
            .unwrap_or_default();
        let kids = self
            .children
            .get(&parent)
            .filter(|kids| !kids.is_empty())
            .ok_or_else(|| FakeError::NoExpansion(parent.clone()))?;

        // Each expansion consumes exactly `k` calls, so the per-label cursor stays
        // a multiple of `kids.len()` between expansions and the modulo always walks
        // the slots in order.
        let slot = {
            let mut cursors = self.cursors.lock().expect("cursor mutex poisoned");
            let cursor = cursors.entry(parent).or_insert(0);
            let slot = *cursor % kids.len();
            *cursor += 1;
            slot
        };
        let child = &kids[slot];

        std::fs::write(worktree.join(FAKE_LABEL_FILE), child)?;
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        std::fs::write(worktree.join(FAKE_SEQ_FILE), seq.to_string())?;

        Ok(TurnResult {
            status: TurnStatus::Edited,
            summary: format!("stamp {child}"),
            changed_paths: vec![PathBuf::from(FAKE_LABEL_FILE), PathBuf::from(FAKE_SEQ_FILE)],
        })
    }
}

pub struct FakeValuer {
    by_label: HashMap<String, Value>,
    default: Value,
}

impl FakeValuer {
    pub fn new(default: Value) -> Self {
        Self {
            by_label: HashMap::new(),
            default,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>, value: Value) -> Self {
        self.by_label.insert(label.into(), value);
        self
    }
}

impl Valuer for FakeValuer {
    type Error = FakeError;

    fn value(&self, tree: &Path) -> Result<Value, Self::Error> {
        match std::fs::read_to_string(tree.join(FAKE_LABEL_FILE)) {
            Ok(raw) => {
                let label = raw.trim();
                self.by_label
                    .get(label)
                    .copied()
                    .ok_or_else(|| FakeError::UnknownLabel(label.to_owned()))
            }
            // No stamped label ⇒ the turn carried no scripted value; fall back.
            Err(_) => Ok(self.default),
        }
    }
}

pub struct FakeMergeOracle {
    resolution: ResolvedState,
}

impl FakeMergeOracle {
    pub fn new(resolution: ResolvedState) -> Self {
        Self { resolution }
    }
}

impl MergeOracle for FakeMergeOracle {
    type Error = FakeError;

    fn adjudicate(
        &self,
        _base: &Path,
        _a: &Path,
        _b: &Path,
        _conflicts: &Conflicts,
    ) -> Result<ResolvedState, Self::Error> {
        Ok(self.resolution.clone())
    }
}

/// A [`Valuer`] that scores a tree by how many Rust source files it holds rather
/// than by a stamped label. The merge tests need a clean two-branch join — one
/// branch adding `a.rs`, the other `b.rs` — to out-value either parent, which a
/// label-keyed [`FakeValuer`] cannot express: the merged tree carries no single
/// parent's [`FAKE_LABEL_FILE`]. Counting source files lets a 2-file merge
/// naturally out-tier its 1-file parents; nothing is ever gated.
#[derive(Default)]
pub struct FileCountValuer;

impl FileCountValuer {
    pub fn new() -> Self {
        Self
    }
}

impl Valuer for FileCountValuer {
    type Error = FakeError;

    fn value(&self, tree: &Path) -> Result<Value, Self::Error> {
        let (tier, score) = match count_rust_files(tree) {
            0 => (GenomeTier::StillLife, 0.0),
            1 => (GenomeTier::Spaceship, 0.5),
            _ => (GenomeTier::Replicator, 0.9),
        };
        Ok(Value {
            gated: false,
            score,
            tier,
        })
    }
}

/// Count the `*.rs` files directly under `dir`, returning 0 for an unreadable
/// worktree. Each merge-test branch adds one top-level source file, so a
/// non-recursive count distinguishes a two-file join from its one-file parents;
/// `.git` links and the `seed.txt` fixture fall out for free on the extension test.
fn count_rust_files(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "rs"))
        .count()
}

/// A [`MergeOracle`] that resolves a conflict by running a scripted [`TurnExecutor`]
/// in the `a` ("ours") worktree, mirroring the production judge oracle's in-place
/// contract: the resolved tree IS `a`, which the engine then commits as the
/// two-parent merge node. Generic over the executor so a test scripts the
/// resolution through a [`FakeTurnExecutor`] with no spawned agent. It lives here
/// — rather than reusing the nit-tui judge oracle, which is unreachable from this
/// crate's `tests/` — so the engine's conflict path is exercisable offline.
/// [`FakeMergeOracle`] only returns a canned tree and cannot edit the worktree, so
/// it cannot drive that path.
pub struct ScriptedMergeOracle<T> {
    judge: T,
}

impl<T: TurnExecutor> ScriptedMergeOracle<T> {
    pub fn new(judge: T) -> Self {
        Self { judge }
    }
}

impl<T: TurnExecutor> MergeOracle for ScriptedMergeOracle<T> {
    type Error = T::Error;

    fn adjudicate(
        &self,
        _base: &Path,
        a: &Path,
        _b: &Path,
        conflicts: &Conflicts,
    ) -> Result<ResolvedState, Self::Error> {
        let task = Task {
            prompt: format!("reconcile {} conflicted path(s)", conflicts.paths.len()),
            role: "merge-judge".to_owned(),
        };
        self.judge.run_turn(a, &task)?;
        Ok(ResolvedState {
            tree: a.to_path_buf(),
        })
    }
}
