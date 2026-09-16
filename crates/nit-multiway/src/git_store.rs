//! The real [`WorldStore`]: content-addressed world states backed by `git`,
//! spawned via [`std::process::Command`] (never a shell). This is the Phase 0
//! centerpiece of the multiway engine.
//!
//! Git-safety model (correctness, not style — these invariants are the contract):
//!
//! - The operator's primary working tree is the READ-ONLY source of the initial
//!   snapshot. The engine never `checkout`s, `reset`s, stages, or commits in it.
//!   All mutation happens in detached worktrees the engine owns, so the live
//!   editor that auto-saves over that tree can never have a buffer clobbered by a
//!   git revert.
//! - Per-turn commits are anchored on throwaway refs under
//!   `refs/nit-multiway/<mission>/…`; worktrees live only under
//!   `<worktrees_root>/<mission>/…`. No branch is ever created on the operator's
//!   branch, and the ref is written *before* the worktree is removed so an
//!   `gc.auto` pass cannot prune the just-made commit.
//! - Commits carry a fixed engine identity passed explicitly per invocation, so
//!   the operator's `user.name`/`user.email` are never used and global git config
//!   is never written. Plumbing (`write-tree` → `commit-tree` → `update-ref`)
//!   runs no hooks and moves no `HEAD`.
//!
//! Frontier width is the caller's concern: every live worktree+turn holds open
//! fds, so a Phase 2/3 caller must clamp `fork`'s `k` by the effective swarm cap
//! (`compute_effective_max_swarm_size`). The store itself forks exactly what it
//! is told and does not import that ceiling.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::node::NodeId;
use crate::traits::{Conflicts, MergeOutcome, WorktreeHandle, WorldStore};

/// `merge-tree --write-tree` (an object-only three-way merge that never touches a
/// working tree) landed in git 2.38; below that, `merge` reports `GitTooOld`.
const MERGE_TREE_MIN: (u32, u32) = (2, 38);

#[derive(Debug, thiserror::Error)]
pub enum GitStoreError {
    #[error("spawning git failed: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("git {cmd} failed: {stderr}")]
    Git { cmd: String, stderr: String },
    #[error("filesystem error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0} is not a git repository")]
    NotARepo(PathBuf),
    #[error("operator repository has an unborn HEAD (no commits to snapshot)")]
    UnbornHead,
    #[error("no state or data directory available for multiway worktrees")]
    StateDirUnavailable,
    #[error("worktrees root {root} overlaps the operator tree {src}")]
    OperatorOverlap { root: PathBuf, src: PathBuf },
    #[error("snapshot source {got} is not the repository this store manages ({expected})")]
    SourceMismatch { expected: PathBuf, got: PathBuf },
    #[error("git {version} predates 2.38; merge needs `merge-tree --write-tree`")]
    GitTooOld { version: String },
}

/// A detached worktree the engine owns. Dropping it removes the worktree
/// (`--force`, since plumbing commits leave the working tree ahead of its
/// detached HEAD) so a panic or early return cannot leak it.
pub struct GitWorktree {
    path: PathBuf,
    parent: NodeId,
    repo: PathBuf,
}

impl WorktreeHandle for GitWorktree {
    fn path(&self) -> &Path {
        &self.path
    }
    fn parent(&self) -> &NodeId {
        &self.parent
    }
}

impl Drop for GitWorktree {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .current_dir(&self.repo)
            .args(["worktree", "remove", "--force"])
            .arg(git_path_arg(&self.path))
            .stdin(Stdio::null())
            .env("GIT_TERMINAL_PROMPT", "0")
            .output();
    }
}

pub struct GitWorldStore {
    repo: PathBuf,
    mission_root: PathBuf,
    mission: String,
    counter: AtomicU64,
    git_version: Option<(u32, u32)>,
}

impl GitWorldStore {
    /// `src` is the operator repository (read-only); `worktrees_root` is the base
    /// the caller resolved (conventionally `state_dir()/multiway/worktrees`).
    /// Both are canonicalized and asserted disjoint so a misconfigured root can
    /// never place a worktree inside the operator tree.
    pub fn new(src: &Path, worktrees_root: &Path, mission: &str) -> Result<Self, GitStoreError> {
        let mission = sanitize(mission);
        let repo = canonical(src)?;
        if !is_git_repo(&repo) {
            return Err(GitStoreError::NotARepo(repo));
        }

        let mission_root = worktrees_root.join(&mission);
        // Stale sweep: a prior crash of this same mission could have leaked
        // worktrees here, so start from a fresh, empty root.
        let _ = fs::remove_dir_all(&mission_root);
        mkdir_all(&mission_root)?;
        let mission_root = canonical(&mission_root)?;

        // A symlinked root could alias the operator tree; canonicalized
        // containment (either direction) is the only check that catches it.
        if mission_root.starts_with(&repo) || repo.starts_with(&mission_root) {
            return Err(GitStoreError::OperatorOverlap {
                root: mission_root,
                src: repo,
            });
        }

        let store = Self {
            git_version: git_version(&repo),
            repo,
            mission_root,
            mission,
            counter: AtomicU64::new(0),
        };
        // Drop the matching refs/admin a prior crash left behind, keeping the
        // freshly-created (empty) mission root in place for this run.
        store.prune_admin();
        Ok(store)
    }

    /// Resolve the conventional worktrees root from the platform state directory
    /// (falling back to the data directory) and construct a store for `mission`.
    pub fn for_mission(src: &Path, mission: &str) -> Result<Self, GitStoreError> {
        let base = nit_utils::paths::state_dir()
            .or_else(nit_utils::paths::data_dir)
            .ok_or(GitStoreError::StateDirUnavailable)?;
        Self::new(src, &base.join("multiway").join("worktrees"), mission)
    }

    /// End-of-mission teardown: remove every worktree under this mission's root
    /// and prune its refs. Idempotent and best-effort — a half-cleaned mission
    /// never panics the caller.
    pub fn cleanup(&self) {
        if self.mission_root.exists() {
            let _ = fs::remove_dir_all(&self.mission_root);
        }
        self.prune_admin();
    }

    /// Prune stale worktree admin entries and delete this mission's refs without
    /// touching the mission root directory itself.
    fn prune_admin(&self) {
        let _ = self.run(
            self.git(&self.repo).args(["worktree", "prune"]),
            "worktree prune",
        );
        if let Ok(listing) = self.run(
            self.git(&self.repo).args([
                "for-each-ref",
                "--format=%(refname)",
                &self.ref_namespace(),
            ]),
            "for-each-ref",
        ) {
            for refname in listing.lines().filter(|l| !l.is_empty()) {
                let _ = self.run(
                    self.git(&self.repo).args(["update-ref", "-d", refname]),
                    "update-ref -d",
                );
            }
        }
    }

    fn ref_namespace(&self) -> String {
        format!("refs/nit-multiway/{}", self.mission)
    }

    /// A git invocation pinned to the fixed engine identity, with hooks, signing,
    /// and credential prompts disabled so a turn can never block or run operator
    /// hooks. `dir` is the working directory (the repo, or a worktree).
    fn git(&self, dir: &Path) -> Command {
        let mut cmd = Command::new("git");
        cmd.current_dir(dir)
            .args(["-c", "user.name=nit-multiway"])
            .args(["-c", "user.email=noreply@nit.tools"])
            .args(["-c", "commit.gpgsign=false"])
            .args(["-c", "core.hooksPath=/dev/null"])
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null());
        cmd
    }

    fn run(&self, cmd: &mut Command, what: &str) -> Result<String, GitStoreError> {
        let out = cmd.output().map_err(GitStoreError::Spawn)?;
        if !out.status.success() {
            return Err(GitStoreError::Git {
                cmd: what.to_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    fn add_detached(&self, at: &NodeId, kind: &str) -> Result<GitWorktree, GitStoreError> {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        let full = at.as_str();
        let short = full.get(..12).unwrap_or(full);
        let leaf = sanitize(&format!("{kind}-{short}-{n}"));
        let path = self.mission_root.join(leaf);

        // `--detach` is mandatory: a bare `worktree add` would create a branch.
        // Argument order is `<path>` then `<commit-ish>`.
        self.run(
            self.git(&self.repo)
                .args(["worktree", "add", "--detach"])
                .arg(git_path_arg(&path))
                .arg(at.as_str()),
            "worktree add",
        )?;

        let path = canonical(&path)?;
        debug_assert!(path.starts_with(&self.mission_root));
        Ok(GitWorktree {
            path,
            parent: at.clone(),
            repo: self.repo.clone(),
        })
    }

    /// Anchor `sha` on a throwaway ref so a `gc` pass cannot reclaim it.
    fn anchor(&self, leaf: &str, sha: &str) -> Result<(), GitStoreError> {
        let refname = format!("{}/{}", self.ref_namespace(), sanitize(leaf));
        self.run(
            self.git(&self.repo).args(["update-ref", &refname, sha]),
            "update-ref",
        )?;
        Ok(())
    }

    fn commit_tree(
        &self,
        tree: &str,
        parents: &[&str],
        message: &str,
    ) -> Result<String, GitStoreError> {
        let mut cmd = self.git(&self.repo);
        cmd.args(["commit-tree", tree]);
        for parent in parents {
            cmd.args(["-p", parent]);
        }
        cmd.args(["-m", message]);
        self.run(&mut cmd, "commit-tree")
    }
}

impl WorldStore for GitWorldStore {
    type Worktree = GitWorktree;
    type Error = GitStoreError;

    /// The initial snapshot is HEAD's *tree* — never a fresh `add`/`stash`, which
    /// would mutate the operator index. A dirty operator tree is therefore
    /// snapshotted at its last commit, by design.
    fn snapshot(&self, src: &Path) -> Result<NodeId, Self::Error> {
        let src = canonical(src)?;
        if src != self.repo {
            return Err(GitStoreError::SourceMismatch {
                expected: self.repo.clone(),
                got: src,
            });
        }
        let sha = self
            .run(
                self.git(&self.repo)
                    .args(["rev-parse", "--verify", "HEAD^{commit}"]),
                "rev-parse HEAD",
            )
            .map_err(|_| GitStoreError::UnbornHead)?;
        self.anchor("root", &sha)?;
        Ok(NodeId::new(sha))
    }

    fn fork(&self, parent: &NodeId, k: usize) -> Result<Vec<Self::Worktree>, Self::Error> {
        let mut worktrees = Vec::with_capacity(k);
        for _ in 0..k {
            // On failure `worktrees` drops here and each handle's `Drop` removes
            // its worktree, so a partial fork unwinds transactionally.
            worktrees.push(self.add_detached(parent, "fork")?);
        }
        Ok(worktrees)
    }

    fn commit(&self, worktree: Self::Worktree, message: &str) -> Result<NodeId, Self::Error> {
        let wt = worktree.path().to_path_buf();
        self.run(self.git(&wt).args(["add", "-A"]), "add")?;
        let tree = self.run(self.git(&wt).arg("write-tree"), "write-tree")?;
        let sha = self.commit_tree(&tree, &[worktree.parent().as_str()], message)?;
        // Ref before the worktree's `Drop` removes it (drop runs at end of scope).
        self.anchor(&sha, &sha)?;
        Ok(NodeId::new(sha))
    }

    /// Clean (non-conflicting) merges only for now; a `Conflict` is a normal
    /// search event the Phase 4 oracle resolves. The object-level
    /// `merge-tree --write-tree` mutates no working tree, so the operator's stays
    /// pristine even though the merge runs in its repo.
    fn merge(&self, a: &NodeId, b: &NodeId) -> Result<MergeOutcome, Self::Error> {
        if let Some(version) = self.git_version {
            if version < MERGE_TREE_MIN {
                return Err(GitStoreError::GitTooOld {
                    version: format!("{}.{}", version.0, version.1),
                });
            }
        }

        let out = self
            .git(&self.repo)
            .args(["merge-tree", "--write-tree"])
            .args([a.as_str(), b.as_str()])
            .output()
            .map_err(GitStoreError::Spawn)?;
        let stdout = String::from_utf8_lossy(&out.stdout);

        match out.status.code() {
            Some(0) => {
                let tree = stdout.lines().next().unwrap_or_default().trim();
                let sha = self.commit_tree(tree, &[a.as_str(), b.as_str()], "multiway merge")?;
                self.anchor(&sha, &sha)?;
                Ok(MergeOutcome::Merged(NodeId::new(sha)))
            }
            Some(1) => Ok(MergeOutcome::Conflict(parse_conflicts(&stdout))),
            _ => Err(GitStoreError::Git {
                cmd: "merge-tree".to_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            }),
        }
    }

    fn commit_merge(
        &self,
        worktree: Self::Worktree,
        a: &NodeId,
        b: &NodeId,
        message: &str,
    ) -> Result<NodeId, Self::Error> {
        let wt = worktree.path().to_path_buf();
        self.run(self.git(&wt).args(["add", "-A"]), "add")?;
        let tree = self.run(self.git(&wt).arg("write-tree"), "write-tree")?;
        // Both parents are passed explicitly; `worktree.parent()` (the `a` side
        // restore handed the oracle) is deliberately ignored so the recorded join
        // never depends on which side was restored.
        let sha = self.commit_tree(&tree, &[a.as_str(), b.as_str()], message)?;
        // Anchor the merge commit before this handle drops at end of scope: no
        // branch points at it, so an unanchored merge is gc-reclaimable.
        self.anchor(&sha, &sha)?;
        Ok(NodeId::new(sha))
    }

    fn merge_supported(&self) -> bool {
        // An unparseable `--version` (`None`) is conservatively unsupported:
        // skipping a merge is always safe, but probing an unknown git is not.
        self.git_version.is_some_and(|v| v >= MERGE_TREE_MIN)
    }

    fn restore(&self, node: &NodeId) -> Result<Self::Worktree, Self::Error> {
        self.add_detached(node, "restore")
    }
}

impl Drop for GitWorldStore {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// On a conflict, `merge-tree` prints the (partial) tree OID, then one
/// `<mode> <oid> <stage>\t<path>` line per conflicted entry, then a blank line
/// before its informational messages. Collect the distinct conflicted paths.
fn parse_conflicts(stdout: &str) -> Conflicts {
    let mut paths = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for line in stdout.lines().skip(1) {
        if line.trim().is_empty() {
            break;
        }
        if let Some((_, path)) = line.split_once('\t') {
            if seen.insert(path.to_owned()) {
                paths.push(PathBuf::from(path));
            }
        }
    }
    Conflicts { paths }
}

fn sanitize(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "x".to_owned()
    } else {
        cleaned
    }
}

/// Strip the Windows verbatim prefix (`\\?\`) that `Path::canonicalize`
/// adds, for paths handed to git as *arguments*. Git for Windows cannot parse
/// that prefix (it becomes `//?/C:/...` and `worktree add` fails with "Invalid
/// argument"). Paths used as `current_dir` need no change. On Unix a path
/// never carries a prefix, so this is a plain copy.
fn git_path_arg(path: &Path) -> PathBuf {
    use std::path::{Component, Prefix};

    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return path.to_path_buf();
    };
    let mut plain = match prefix.kind() {
        Prefix::VerbatimDisk(letter) => PathBuf::from(format!("{}:\\", letter as char)),
        Prefix::VerbatimUNC(server, share) => {
            let mut unc = std::ffi::OsString::from(r"\\");
            unc.push(server);
            unc.push(r"\");
            unc.push(share);
            unc.push(r"\");
            PathBuf::from(unc)
        }
        _ => return path.to_path_buf(),
    };
    for component in components {
        if let Component::Normal(part) = component {
            plain.push(part);
        }
    }
    plain
}

fn canonical(path: &Path) -> Result<PathBuf, GitStoreError> {
    path.canonicalize().map_err(|source| GitStoreError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn mkdir_all(path: &Path) -> Result<(), GitStoreError> {
    fs::create_dir_all(path).map_err(|source| GitStoreError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn is_git_repo(dir: &Path) -> bool {
    Command::new("git")
        .current_dir(dir)
        .args(["rev-parse", "--git-dir"])
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn git_version(dir: &Path) -> Option<(u32, u32)> {
    let out = Command::new("git")
        .current_dir(dir)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let raw = String::from_utf8_lossy(&out.stdout);
    let version = raw.split_whitespace().nth(2)?;
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::git_path_arg;
    use std::path::Path;

    #[test]
    fn git_path_arg_leaves_plain_paths_alone() {
        let plain = if cfg!(windows) {
            Path::new(r"C:\Users\me\wt")
        } else {
            Path::new("/tmp/nit/wt")
        };
        assert_eq!(git_path_arg(plain), plain);
    }

    #[cfg(windows)]
    #[test]
    fn git_path_arg_strips_verbatim_disk_prefix() {
        let verbatim = Path::new(r"\\?\C:\Users\me\wt");
        assert_eq!(git_path_arg(verbatim), Path::new(r"C:\Users\me\wt"));
    }

    #[cfg(windows)]
    #[test]
    fn git_path_arg_strips_verbatim_unc_prefix() {
        let verbatim = Path::new(r"\\?\UNC\server\share\dir\wt");
        assert_eq!(git_path_arg(verbatim), Path::new(r"\\server\share\dir\wt"));
    }
}
