//! Phase 3 acceptance — the multiway `TurnExecutor` turn-result contract and the
//! `NoMergeOracle` placeholder (`nit_tui::multiway::executor`).
//!
//! The production executor, `RunnerExecutor`, drives a spawned `claude` through a
//! dedicated `ClaudeRunner`; that runner's event channel has no offline injection
//! seam, so a deterministic CI test of `RunnerExecutor::run_turn` would have to mock
//! the runner — the over-engineering the build contract ruled out. What every
//! `TurnExecutor` must guarantee is independent of the backend, and the engine's
//! expansion step depends only on that contract: a turn that writes files reports
//! `Edited` with the changed paths, a turn that writes nothing reports `NoOp`, a
//! dead turn reports `Failed` (an `Ok` result, never an `Err` — a dead branch is a
//! normal search event, not an engine abort), and a turn mutates only the worktree
//! it was handed. The scripted `FakeTurnExecutor` reproduces exactly that contract,
//! and is the same backend the engine's own loop runs against.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use nit_multiway::testing::{FakeTurnExecutor, ScriptedTurn};
use nit_multiway::traits::{Task, TurnExecutor, TurnStatus};

struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("nit_mw_exec_{label}_{}_{n}", std::process::id()));
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

fn task() -> Task {
    Task {
        prompt: "scripted multiway turn".to_owned(),
        role: "integrate".to_owned(),
    }
}

/// Relative path + bytes of every file under `root`, sorted — a probe for whether a
/// turn wrote, or escaped, the tree it was handed.
fn manifest(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).expect("read_dir").flatten() {
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

#[test]
fn turn_results_map_status_changed_paths_and_writes() {
    let worktree = ScratchDir::new("status");
    // One executor, three scripted turns — the engine drives a backend exactly this
    // way, one in-flight turn at a time, the cursor advancing per call.
    let executor = FakeTurnExecutor::new(vec![
        ScriptedTurn {
            status: TurnStatus::Edited,
            summary: "wrote the module".to_owned(),
            writes: vec![
                (PathBuf::from("src/lib.rs"), "pub fn f() {}\n".to_owned()),
                (PathBuf::from("README.md"), "# scratch\n".to_owned()),
            ],
            label: None,
        },
        ScriptedTurn {
            status: TurnStatus::NoOp,
            summary: "nothing to change".to_owned(),
            writes: Vec::new(),
            label: None,
        },
        ScriptedTurn {
            status: TurnStatus::Failed,
            summary: "the turn died".to_owned(),
            writes: Vec::new(),
            label: None,
        },
    ]);
    let scratch = task();

    // Edited: reports precisely the paths it wrote, and they land on disk for the
    // engine to commit.
    let edited = executor
        .run_turn(worktree.path(), &scratch)
        .expect("edited turn runs");
    assert_eq!(edited.status, TurnStatus::Edited);
    assert_eq!(
        edited.changed_paths,
        vec![PathBuf::from("src/lib.rs"), PathBuf::from("README.md")]
    );
    assert!(worktree.path().join("src/lib.rs").exists());
    assert!(worktree.path().join("README.md").exists());

    // NoOp: no reported changes, and the tree is exactly as the edit left it — so
    // the engine commits no child for it.
    let settled = manifest(worktree.path());
    let noop = executor
        .run_turn(worktree.path(), &scratch)
        .expect("noop turn runs");
    assert_eq!(noop.status, TurnStatus::NoOp);
    assert!(noop.changed_paths.is_empty());
    assert_eq!(settled, manifest(worktree.path()));

    // Failed: a dead branch surfaces as an `Ok(Failed)` the search drops, never a
    // propagated error (`expect` would have panicked on an `Err`).
    let failed = executor
        .run_turn(worktree.path(), &scratch)
        .expect("failed turn is still Ok");
    assert_eq!(failed.status, TurnStatus::Failed);
    assert!(failed.changed_paths.is_empty());
}

#[test]
fn turn_touches_only_the_supplied_worktree() {
    let worktree = ScratchDir::new("wt");
    let bystander = ScratchDir::new("bystander");
    // The bystander stands in for the operator's primary tree: the turn must never
    // reach outside the worktree it was handed.
    fs::write(bystander.path().join("operator.rs"), "fn main() {}\n").expect("seed bystander");
    let untouched = manifest(bystander.path());

    let result = FakeTurnExecutor::new(vec![ScriptedTurn {
        status: TurnStatus::Edited,
        summary: "edit confined to the worktree".to_owned(),
        writes: vec![(
            PathBuf::from("candidate.rs"),
            "pub fn g() -> u8 { 1 }\n".to_owned(),
        )],
        label: None,
    }])
    .run_turn(worktree.path(), &task())
    .expect("scripted turn runs");

    assert_eq!(result.status, TurnStatus::Edited);
    assert!(
        worktree.path().join("candidate.rs").exists(),
        "the edit landed in the worktree"
    );
    assert_eq!(
        untouched,
        manifest(bystander.path()),
        "the turn escaped its worktree and mutated the bystander tree"
    );
}

#[test]
fn no_merge_oracle_errs_if_reached() {
    use nit_multiway::traits::{Conflicts, MergeOracle};
    use nit_tui::multiway::executor::{ExecutorError, NoMergeOracle};

    // Merge is a Phase-4 feature the Phase 2/3 search loop never triggers, so the
    // placeholder fails loudly rather than silently returning a wrong resolution.
    let here = Path::new(".");
    let err = NoMergeOracle
        .adjudicate(here, here, here, &Conflicts::default())
        .expect_err("the v1 loop must never adjudicate a merge");
    assert!(matches!(err, ExecutorError::MergeUnsupported));
}

#[test]
fn judge_merge_oracle_writes_resolution_into_the_a_tree() {
    use nit_multiway::traits::{Conflicts, MergeOracle};
    use nit_tui::multiway::executor::JudgeMergeOracle;

    // The three restored merge-base trees the engine hands the oracle on a
    // conflict: OURS and THEIRS both changed `same.rs` relative to BASE, so git
    // could not auto-merge it. A real run would drive a spawned `claude`; here a
    // scripted `FakeTurnExecutor` stands in for that judge with no process spawn.
    let base = ScratchDir::new("merge_base");
    let ours = ScratchDir::new("merge_ours");
    let theirs = ScratchDir::new("merge_theirs");
    fs::write(base.path().join("same.rs"), "pub fn v() -> u8 { 0 }\n").expect("seed base");
    fs::write(ours.path().join("same.rs"), "pub fn v() -> u8 { 1 }\n").expect("seed ours");
    fs::write(theirs.path().join("same.rs"), "pub fn v() -> u8 { 2 }\n").expect("seed theirs");
    let theirs_before = manifest(theirs.path());

    let resolved_body = "pub fn v() -> u8 { 3 }\n";
    let oracle = JudgeMergeOracle::new(FakeTurnExecutor::new(vec![ScriptedTurn {
        status: TurnStatus::Edited,
        summary: "reconciled same.rs".to_owned(),
        writes: vec![(PathBuf::from("same.rs"), resolved_body.to_owned())],
        label: None,
    }]));

    let conflicts = Conflicts {
        paths: vec![PathBuf::from("same.rs")],
    };
    let resolved = oracle
        .adjudicate(base.path(), ours.path(), theirs.path(), &conflicts)
        .expect("the scripted judge resolves the conflict");

    // In-place contract: the resolved tree IS the `a`/OURS worktree, and it now
    // holds the merged content the engine will commit as the two-parent node.
    assert_eq!(resolved.tree.as_path(), ours.path());
    assert_eq!(
        fs::read_to_string(ours.path().join("same.rs")).expect("merged file"),
        resolved_body,
        "the adjudicated resolution landed in the a/OURS worktree"
    );
    // Adjudication runs only in OURS — the sibling tree it was shown is untouched.
    assert_eq!(
        theirs_before,
        manifest(theirs.path()),
        "the oracle mutated a tree other than the one it resolves into"
    );
}
