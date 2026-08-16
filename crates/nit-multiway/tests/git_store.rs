//! Phase 0 acceptance for [`GitWorldStore`]: on a scratch repo in a temp dir,
//! fork a node into k worktrees, commit a scripted edit in each, merge two
//! non-conflicting nodes, restore an older node — and prove the operator's
//! primary tree is never touched.
//!
//! The operator-untouched proof is the load-bearing assertion: it captures
//! HEAD, the symbolic HEAD (detached-vs-branch), `status --porcelain`, the
//! head/tag/remote refs, and a content+mtime manifest of the whole operator cwd
//! *excluding only `.git/`* — so a stray write to a `.gitignore`d file (the live
//! editor-buffer threat the engine exists to avoid) would fail the test.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use nit_multiway::git_store::{GitStoreError, GitWorldStore};
use nit_multiway::testing::{FakeTurnExecutor, ScriptedTurn};
use nit_multiway::traits::{
    MergeOutcome, Task, TurnExecutor, TurnStatus, WorktreeHandle, WorldStore,
};

struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("nit_mw_{label}_{}_{n}", std::process::id()));
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
        .map(|o| o.status.success())
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

/// A content+mtime fingerprint of every file under `root` except the git dir.
/// `.gitignore`d files are deliberately included.
type FileFingerprint = (String, std::time::SystemTime, blake3::Hash);

fn manifest(root: &Path) -> Vec<FileFingerprint> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<FileFingerprint>) {
        for entry in fs::read_dir(dir).expect("read_dir").flatten() {
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            let meta = entry.metadata().expect("metadata");
            if meta.is_dir() {
                walk(root, &path, out);
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .expect("strip prefix")
                .to_string_lossy()
                .into_owned();
            let body = fs::read(&path).expect("read file");
            out.push((
                relative,
                meta.modified().expect("mtime"),
                blake3::hash(&body),
            ));
        }
    }

    let mut prints = Vec::new();
    walk(root, root, &mut prints);
    prints.sort_by(|left, right| left.0.cmp(&right.0));
    prints
}

/// Everything about the operator repo the engine must leave byte-for-byte intact.
/// `head_ref` catches a stray detach; `files` is a manifest of every working file
/// except `.git/`, so it also catches writes to `.gitignore`d (live-buffer) files.
#[derive(Debug, PartialEq)]
struct OperatorState {
    commit: String,
    head_ref: String,
    porcelain: Vec<u8>,
    refs: String,
    files: Vec<FileFingerprint>,
}

impl OperatorState {
    fn capture(repo: &Path) -> Self {
        let scopes = ["for-each-ref", "refs/heads", "refs/tags", "refs/remotes"];
        Self {
            commit: git_text(repo, &["rev-parse", "HEAD"]),
            head_ref: git_text(repo, &["symbolic-ref", "HEAD"]),
            porcelain: git_bytes(repo, &["status", "--porcelain=v1", "-z"]),
            refs: git_text(repo, &scopes),
            files: manifest(repo),
        }
    }
}

fn init_operator_repo(dir: &Path) {
    git_text(dir, &["init", "-b", "main"]);
    // The operator configures their own repo locally; the engine uses its own
    // fixed identity and never reads or writes this config.
    git_text(dir, &["config", "user.name", "operator"]);
    git_text(dir, &["config", "user.email", "operator@example.com"]);
    fs::write(dir.join("seed.txt"), "seed-content\n").expect("write seed");
    fs::write(dir.join(".gitignore"), "ignored.txt\n").expect("write gitignore");
    // An untracked, ignored file stands in for a live editor buffer: the engine
    // must never disturb it, and the manifest covers it because git status won't.
    fs::write(dir.join("ignored.txt"), "live-buffer\n").expect("write ignored");
    git_text(dir, &["add", "-A"]);
    git_text(dir, &["commit", "-m", "initial"]);
}

#[test]
fn fork_commit_merge_restore_leaves_operator_tree_untouched() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }

    let operator = ScratchDir::new("op");
    let op = operator.path();
    init_operator_repo(op);

    let pristine = OperatorState::capture(op);

    let wt_home = ScratchDir::new("wt");
    let mission = "mis-phase0";
    let store = GitWorldStore::new(op, wt_home.path(), mission).expect("construct store");
    let mission_root = fs::canonicalize(wt_home.path().join(mission)).expect("mission root");

    let root = store.snapshot(op).expect("snapshot");

    // Fork into two worktrees, each isolated under the mission root.
    let mut worktrees = store.fork(&root, 2).expect("fork");
    assert_eq!(worktrees.len(), 2);
    for wt in &worktrees {
        assert!(
            wt.path().starts_with(&mission_root),
            "worktree escaped root"
        );
        assert!(
            wt.path().join("seed.txt").exists(),
            "fork lacks seed content"
        );
        assert_eq!(wt.parent(), &root);
    }

    // Two non-conflicting scripted edits: a.txt in the first, b.txt in the second.
    let executor = FakeTurnExecutor::new(vec![
        ScriptedTurn {
            status: TurnStatus::Edited,
            summary: "add a".to_owned(),
            writes: vec![(PathBuf::from("a.txt"), "alpha\n".to_owned())],
            label: None,
        },
        ScriptedTurn {
            status: TurnStatus::Edited,
            summary: "add b".to_owned(),
            writes: vec![(PathBuf::from("b.txt"), "beta\n".to_owned())],
            label: None,
        },
    ]);
    let task = Task {
        prompt: "scripted".to_owned(),
        role: "test".to_owned(),
    };
    let wt_b = worktrees.pop().unwrap();
    let wt_a = worktrees.pop().unwrap();
    let a_result = executor.run_turn(wt_a.path(), &task).expect("run a");
    let b_result = executor.run_turn(wt_b.path(), &task).expect("run b");
    assert_eq!(a_result.changed_paths, vec![PathBuf::from("a.txt")]);
    assert_eq!(b_result.changed_paths, vec![PathBuf::from("b.txt")]);

    let node_a = store.commit(wt_a, "edit a").expect("commit a");
    let node_b = store.commit(wt_b, "edit b").expect("commit b");
    assert_ne!(node_a, node_b);

    // Merge the two non-conflicting nodes; the result holds both edits + seed.
    let merged = match store.merge(&node_a, &node_b).expect("merge") {
        MergeOutcome::Merged(id) => id,
        MergeOutcome::Conflict(c) => panic!("expected a clean merge, got conflict: {c:?}"),
    };
    let merged_wt = store.restore(&merged).expect("restore merged");
    for name in ["seed.txt", "a.txt", "b.txt"] {
        assert!(
            merged_wt.path().join(name).exists(),
            "merged tree missing {name}"
        );
    }
    drop(merged_wt);

    // Backtrack: restoring the root recovers the original tree, edits absent.
    let root_wt = store.restore(&root).expect("restore root");
    assert!(root_wt.path().join("seed.txt").exists());
    assert!(!root_wt.path().join("a.txt").exists());
    assert!(!root_wt.path().join("b.txt").exists());
    drop(root_wt);

    // Cleanup empties the mission's worktrees and prunes its refs.
    store.cleanup();
    let leftover_worktrees = mission_root.exists()
        && fs::read_dir(&mission_root)
            .expect("read mission root")
            .next()
            .is_some();
    assert!(!leftover_worktrees, "worktrees remained after cleanup");
    let leftover_refs = git_text(op, &["for-each-ref", "refs/nit-multiway"]);
    assert!(leftover_refs.is_empty(), "refs remained: {leftover_refs}");

    // The whole point: the operator's primary tree is byte-for-byte untouched.
    assert_eq!(
        pristine,
        OperatorState::capture(op),
        "the engine mutated the operator's primary tree"
    );
}

#[test]
fn commit_merge_records_two_parent_node_and_cleans_up() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }

    let operator = ScratchDir::new("merge-op");
    let op = operator.path();
    init_operator_repo(op);

    let pristine = OperatorState::capture(op);

    let wt_home = ScratchDir::new("merge-wt");
    let mission = "mis-merge";
    let store = GitWorldStore::new(op, wt_home.path(), mission).expect("construct store");

    // commit_merge only runs after a conflict surfaced by `merge`, so a git too
    // old for `merge-tree --write-tree` degrades to fork-only — skip, don't fail.
    if !store.merge_supported() {
        eprintln!("skipping: git predates merge-tree --write-tree (2.38)");
        return;
    }

    let mission_root = fs::canonicalize(wt_home.path().join(mission)).expect("mission root");
    let root = store.snapshot(op).expect("snapshot");

    // Two siblings of the root rewrite the SAME file divergently → a real
    // conflict the oracle must adjudicate before commit_merge records the join.
    let mut worktrees = store.fork(&root, 2).expect("fork");
    let executor = FakeTurnExecutor::new(vec![
        ScriptedTurn {
            status: TurnStatus::Edited,
            summary: "a rewrites seed".to_owned(),
            writes: vec![(PathBuf::from("seed.txt"), "alpha\n".to_owned())],
            label: None,
        },
        ScriptedTurn {
            status: TurnStatus::Edited,
            summary: "b rewrites seed".to_owned(),
            writes: vec![(PathBuf::from("seed.txt"), "beta\n".to_owned())],
            label: None,
        },
    ]);
    let task = Task {
        prompt: "scripted".to_owned(),
        role: "test".to_owned(),
    };
    let wt_b = worktrees.pop().unwrap();
    let wt_a = worktrees.pop().unwrap();
    executor.run_turn(wt_a.path(), &task).expect("run a");
    executor.run_turn(wt_b.path(), &task).expect("run b");
    let node_a = store.commit(wt_a, "edit a").expect("commit a");
    let node_b = store.commit(wt_b, "edit b").expect("commit b");

    // The object-merge path must report this divergence as a conflict.
    let conflicts = match store.merge(&node_a, &node_b).expect("merge") {
        MergeOutcome::Conflict(c) => c,
        MergeOutcome::Merged(id) => panic!("expected a conflict, got clean merge {id}"),
    };
    assert!(
        conflicts.paths.contains(&PathBuf::from("seed.txt")),
        "conflict set should name seed.txt, got {:?}",
        conflicts.paths
    );

    // restore() materializes base/a/b for the oracle; each must be its own tree.
    let base_wt = store.restore(&root).expect("restore base");
    let a_wt = store.restore(&node_a).expect("restore a");
    let b_wt = store.restore(&node_b).expect("restore b");
    let read_seed = |wt: &Path| fs::read_to_string(wt.join("seed.txt")).expect("read seed");
    assert_eq!(read_seed(base_wt.path()), "seed-content\n");
    assert_eq!(read_seed(a_wt.path()), "alpha\n");
    assert_eq!(read_seed(b_wt.path()), "beta\n");
    drop(base_wt);
    drop(b_wt);

    // Stand in for the oracle resolving the conflict in place in the `a` worktree.
    fs::write(a_wt.path().join("seed.txt"), "resolved\n").expect("write resolution");
    let merged = store
        .commit_merge(a_wt, &node_a, &node_b, "multiway merge (adjudicated)")
        .expect("commit_merge");
    assert_ne!(merged, node_a);
    assert_ne!(merged, node_b);

    // A real two-parent commit: `%P` prints exactly node_a then node_b, and the
    // committer is the fixed engine identity, never the operator's config.
    let parents = git_text(op, &["show", "-s", "--format=%P", merged.as_str()]);
    let parents: Vec<&str> = parents.split_whitespace().collect();
    assert_eq!(
        parents,
        vec![node_a.as_str(), node_b.as_str()],
        "merge parents"
    );
    let committer = git_text(op, &["show", "-s", "--format=%cn", merged.as_str()]);
    assert_eq!(
        committer.trim(),
        "nit-multiway",
        "merge used operator identity"
    );

    // The committed tree is the oracle's resolution (base seed replaced in place).
    let merged_wt = store.restore(&merged).expect("restore merged");
    assert_eq!(read_seed(merged_wt.path()), "resolved\n");
    drop(merged_wt);

    // The merge node is anchored on a ref before cleanup so gc cannot reclaim it.
    let anchored = git_text(
        op,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "refs/nit-multiway",
        ],
    );
    assert!(
        anchored.lines().any(|sha| sha == merged.as_str()),
        "merge commit not anchored under refs/nit-multiway: {anchored}"
    );

    // Cleanup prunes every mission ref and empties the worktree root.
    store.cleanup();
    assert!(
        git_text(op, &["for-each-ref", "refs/nit-multiway"]).is_empty(),
        "refs survived cleanup"
    );
    let leftover = mission_root.exists()
        && fs::read_dir(&mission_root)
            .expect("read mission root")
            .next()
            .is_some();
    assert!(!leftover, "worktrees survived cleanup");

    // And the operator's primary tree is byte-for-byte untouched throughout.
    assert_eq!(
        pristine,
        OperatorState::capture(op),
        "the engine mutated the operator's primary tree"
    );
}

#[test]
fn snapshot_rejects_unborn_head() {
    if !git_present() {
        eprintln!("skipping: git not found on PATH");
        return;
    }
    let operator = ScratchDir::new("unborn-op");
    git_text(operator.path(), &["init", "-b", "main"]);
    let wt_home = ScratchDir::new("unborn-wt");
    let store =
        GitWorldStore::new(operator.path(), wt_home.path(), "unborn").expect("construct store");
    let err = store
        .snapshot(operator.path())
        .expect_err("unborn HEAD must error");
    assert!(matches!(err, GitStoreError::UnbornHead), "got {err:?}");
}
