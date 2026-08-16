//! Real-repo smoke for the multiway `GitWorldStore`: exercises the Phase-0
//! primitives (snapshot → fork → commit → merge → restore → cleanup) against a
//! real git repository and proves the operator's working tree is byte-for-byte
//! untouched. Complements `tests/git_store.rs` (a scratch repo) by running on a
//! large real repo on demand.
//!
//!   cargo run -p nit-multiway --example store_smoke -- [repo-path]
//!
//! `repo-path` defaults to the current directory. The store writes only under a
//! temp worktrees root and throwaway `refs/nit-multiway/…`, then cleans up.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

use nit_multiway::git_store::GitWorldStore;
use nit_multiway::traits::{MergeOutcome, WorktreeHandle, WorldStore};

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("spawn git");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// HEAD commit + porcelain status: the operator-state fingerprint we require to
/// be identical before and after the engine runs.
fn operator_state(repo: &Path) -> (String, String) {
    (
        git(repo, &["rev-parse", "HEAD"]),
        git(repo, &["status", "--porcelain"]),
    )
}

fn main() -> Result<(), Box<dyn Error>> {
    let repo = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let repo = repo.canonicalize()?;
    let worktrees_root = std::env::temp_dir().join(format!("nit_mw_smoke_{}", std::process::id()));
    println!("repo            : {}", repo.display());
    println!("worktrees root  : {}", worktrees_root.display());

    let before = operator_state(&repo);

    let store = GitWorldStore::new(&repo, &worktrees_root, "smoke")?;
    let root = store.snapshot(&repo)?;
    println!("snapshot (root) : {}", root.as_str());

    // Fork into two isolated worktrees; make a non-conflicting edit in each.
    let mut worktrees = store.fork(&root, 2)?;
    println!("forked          : {} worktrees", worktrees.len());
    let wt_b = worktrees.pop().ok_or("fork returned < 2 worktrees")?;
    let wt_a = worktrees.pop().ok_or("fork returned < 2 worktrees")?;
    let iso_root = worktrees_root.canonicalize()?;
    for wt in [&wt_a, &wt_b] {
        if !wt.path().starts_with(&iso_root) {
            return Err("worktree escaped the isolation root".into());
        }
    }
    std::fs::write(wt_a.path().join("smoke_a.txt"), "alpha\n")?;
    std::fs::write(wt_b.path().join("smoke_b.txt"), "beta\n")?;
    let node_a = store.commit(wt_a, "smoke: edit a")?;
    let node_b = store.commit(wt_b, "smoke: edit b")?;
    println!(
        "committed       : a={} b={}",
        short(node_a.as_str()),
        short(node_b.as_str())
    );

    // Adjudicated-free merge: two non-conflicting branches join cleanly.
    let merged = match store.merge(&node_a, &node_b)? {
        MergeOutcome::Merged(id) => id,
        MergeOutcome::Conflict(c) => return Err(format!("unexpected conflict: {c:?}").into()),
    };
    let merged_wt = store.restore(&merged)?;
    let has_both = merged_wt.path().join("smoke_a.txt").exists()
        && merged_wt.path().join("smoke_b.txt").exists();
    println!(
        "merged          : {} (holds both edits: {has_both})",
        short(merged.as_str())
    );
    drop(merged_wt);

    // Backtrack: the root tree has neither edit.
    let root_wt = store.restore(&root)?;
    let backtrack_clean = !root_wt.path().join("smoke_a.txt").exists()
        && !root_wt.path().join("smoke_b.txt").exists();
    println!("restored root   : edits absent: {backtrack_clean}");
    drop(root_wt);

    store.cleanup();
    let after = operator_state(&repo);
    let untouched = before == after;
    let leftover_refs = git(&repo, &["for-each-ref", "refs/nit-multiway"]);

    println!("---");
    println!("operator tree untouched : {untouched}");
    println!("merge held both edits   : {has_both}");
    println!("backtrack recovered root: {backtrack_clean}");
    println!("refs cleaned up         : {}", leftover_refs.is_empty());

    if untouched && has_both && backtrack_clean && leftover_refs.is_empty() {
        println!("SMOKE: PASS");
        Ok(())
    } else {
        Err("SMOKE: FAIL — see flags above".into())
    }
}

fn short(sha: &str) -> &str {
    sha.get(..10).unwrap_or(sha)
}
