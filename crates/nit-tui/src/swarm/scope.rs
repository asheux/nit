use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Hard upper bound on entries returned to the planner. The walk also early-
/// exits the moment this is reached so we never visit more dirs than needed.
const SCOPE_WALK_MAX_FILES: usize = 100;

/// Hard cap on directory recursion depth — defense against pathological deep
/// trees even when the wrapping deadline is generous.
const SCOPE_WALK_MAX_DEPTH: usize = 12;

/// Default deadline waited on the foreground thread for the background walk
/// to complete. If the walk exceeds this, dispatch proceeds with an empty
/// scope (same behavior as if no path tokens were detected). The walker
/// thread keeps running in the background and will exit on its own once it
/// finishes or hits its size/depth caps.
const SCOPE_WALK_DEFAULT_TIMEOUT_MS: u64 = 200;

/// Directories never descended into. The existing `.*` skip already covers
/// `.git`, `.cache`, etc.; this list extends it to common source mounds that
/// would balloon the walk for no useful planner signal.
const SCOPE_WALK_SKIP_DIRS: &[&str] = &["target", "node_modules"];

pub(super) fn sanitize_for_filename(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Extract directory/module paths from the operator prompt and enumerate their
/// source files.  Returns relative paths sorted alphabetically, capped at
/// `SCOPE_WALK_MAX_FILES`.
///
/// The walk runs on a background thread and the foreground thread waits up to
/// `scope_walk_timeout()` (default 200 ms, override with the
/// `NIT_SCOPE_WALK_TIMEOUT_MS` env var). If the walk is slower than that —
/// big trees, slow disks, or pathological structures — the foreground returns
/// an empty `Vec` immediately so the UI never freezes. The walker thread
/// keeps draining and exits on its own; its result is discarded since the
/// channel receiver has already been dropped.
///
/// When the prompt contains no usable path tokens (no token with `/`), the
/// walk falls back to `git diff --name-only` against the merge-base with
/// `main` (or `master`) so the verifier can still scope to the operator's
/// in-progress edits instead of running `--workspace --all-features`.
/// Returns empty if the workspace isn't a git repo or git is missing.
pub(crate) fn enumerate_scope_files(workspace_root: &Path, prompt: &str) -> Vec<String> {
    enumerate_scope_files_with_deadline(workspace_root, prompt, scope_walk_timeout())
}

pub(crate) fn enumerate_scope_files_with_deadline(
    workspace_root: &Path,
    prompt: &str,
    deadline: Duration,
) -> Vec<String> {
    // A zero deadline means "skip the walk". `recv_timeout(Duration::ZERO)`
    // races with the worker on fast machines (the walker can deliver a
    // result before the timeout fires), so short-circuit before spawning.
    if deadline.is_zero() {
        return Vec::new();
    }
    let workspace_root = workspace_root.to_path_buf();
    let prompt = prompt.to_string();
    let (tx, rx) = mpsc::channel();

    if thread::Builder::new()
        .name("nit-scope-walk".into())
        .spawn(move || {
            let result = enumerate_scope_files_blocking(&workspace_root, &prompt);
            let _ = tx.send(result);
        })
        .is_err()
    {
        // Spawn failure (e.g. fd exhaustion). Fall back to no-scope rather
        // than blocking the UI thread on a synchronous walk.
        return Vec::new();
    }

    rx.recv_timeout(deadline).unwrap_or_default()
}

/// Synchronous implementation. Bounded by `SCOPE_WALK_MAX_FILES` and
/// `SCOPE_WALK_MAX_DEPTH`, and never follows symlinks (so a self-referential
/// or upward-pointing symlink can't spin the walk forever).
fn enumerate_scope_files_blocking(workspace_root: &Path, prompt: &str) -> Vec<String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for token in prompt.split_whitespace() {
        let token = token.trim_matches(|c: char| c == ',' || c == '.' || c == '"' || c == '\'');
        if token.is_empty() {
            continue;
        }
        if !token.contains('/') {
            continue;
        }
        let candidate = workspace_root.join(token);
        if candidate.is_dir() {
            dirs.push(candidate);
        }
    }
    if dirs.is_empty() {
        // Prompt had no usable path tokens. Fall back to whatever the
        // operator has actually changed in the working tree so the
        // verifier still runs scoped commands (`-p <pkg>`) instead of
        // collapsing to `--workspace --all-features`.
        return git_changed_scope_files(workspace_root);
    }

    let mut files = Vec::new();
    for dir in dirs.iter() {
        collect_source_files(dir, workspace_root, &mut files, 0);
        if files.len() >= SCOPE_WALK_MAX_FILES {
            break;
        }
    }
    files.sort();
    files.dedup();
    files.truncate(SCOPE_WALK_MAX_FILES);
    files
}

/// Last-resort scope source: ask git which files have changed in the
/// current branch (committed + staged + unstaged), starting from the
/// merge-base with `main`/`master`. Returns paths relative to
/// `workspace_root`, filtered to the same source-file extensions the
/// directory walk uses, and capped at `SCOPE_WALK_MAX_FILES`.
///
/// Returns `Vec::new()` on any failure: not a git repo, git missing,
/// no merge-base, command nonzero exit, garbled output. Callers treat
/// an empty result the same as "no scope" — verifier falls back to
/// unscoped (`--workspace`) commands.
fn git_changed_scope_files(workspace_root: &Path) -> Vec<String> {
    // Pick the diff base. Prefer `main`, fall back to `master`. If
    // neither exists, fall back to plain `HEAD` so we still catch
    // uncommitted edits.
    let base = git_diff_base(workspace_root).unwrap_or_else(|| "HEAD".to_string());

    let output = Command::new("git")
        .args(["diff", "--name-only", &base])
        .current_dir(workspace_root)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let stdout = match String::from_utf8(output.stdout) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let mut files: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            // Match the directory walk's accepted extensions.
            std::path::Path::new(line)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| {
                    matches!(
                        ext,
                        "rs" | "toml" | "ts" | "js" | "py" | "go" | "c" | "h" | "cpp" | "hpp"
                    )
                })
        })
        .filter(|line| {
            // Skip files in directories the walk skips, so the two
            // sources stay consistent.
            !line.split('/').any(|component| {
                component.starts_with('.') || SCOPE_WALK_SKIP_DIRS.contains(&component)
            })
        })
        .filter(|line| {
            // Final sanity check: file must currently exist on disk
            // (a deletion would be in the diff but produce no
            // verifier-relevant scope).
            workspace_root.join(line).exists()
        })
        .map(String::from)
        .collect();
    files.sort();
    files.dedup();
    files.truncate(SCOPE_WALK_MAX_FILES);
    files
}

/// Try to find a diff base by asking git for the merge-base of
/// `HEAD` against `main`, then `master`. Returns `None` if neither
/// branch exists or git fails. Caller treats `None` as "no committed
/// branch base" and falls back to `HEAD` (uncommitted only).
fn git_diff_base(workspace_root: &Path) -> Option<String> {
    for branch in ["main", "master"] {
        let output = Command::new("git")
            .args(["merge-base", "HEAD", branch])
            .current_dir(workspace_root)
            .output()
            .ok()?;
        if !output.status.success() {
            continue;
        }
        let sha = String::from_utf8(output.stdout).ok()?.trim().to_string();
        if !sha.is_empty() {
            return Some(sha);
        }
    }
    None
}

fn collect_source_files(dir: &Path, workspace_root: &Path, out: &mut Vec<String>, depth: usize) {
    if depth >= SCOPE_WALK_MAX_DEPTH {
        return;
    }
    if out.len() >= SCOPE_WALK_MAX_FILES {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= SCOPE_WALK_MAX_FILES {
            return;
        }
        // `symlink_metadata` does not follow symlinks: any symlink (including
        // a self-loop or one pointing back to an ancestor) shows up as a
        // symlink, not as the dir/file it points at, so we skip it. This is
        // our cycle guard — cheap and traversal-free.
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        if meta.is_symlink() {
            continue;
        }
        let path = entry.path();
        if meta.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') || SCOPE_WALK_SKIP_DIRS.contains(&name) {
                    continue;
                }
            }
            collect_source_files(&path, workspace_root, out, depth + 1);
        } else if meta.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(
                    ext,
                    "rs" | "toml" | "ts" | "js" | "py" | "go" | "c" | "h" | "cpp" | "hpp"
                ) {
                    if let Ok(rel) = path.strip_prefix(workspace_root) {
                        out.push(rel.display().to_string());
                    }
                }
            }
        }
    }
}

fn scope_walk_timeout() -> Duration {
    let default = Duration::from_millis(SCOPE_WALK_DEFAULT_TIMEOUT_MS);
    match std::env::var("NIT_SCOPE_WALK_TIMEOUT_MS") {
        Ok(raw) => {
            let raw = raw.trim();
            if raw.is_empty() {
                return default;
            }
            match raw.parse::<u64>() {
                // 0 means "skip the walk entirely" — return-immediately
                // semantic. The worker thread is still spawned but the
                // foreground returns Vec::new() right away.
                Ok(0) => Duration::ZERO,
                Ok(ms) => Duration::from_millis(ms),
                Err(_) => default,
            }
        }
        Err(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn fresh_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("nit-scope-test-{}-{}", name, std::process::id(),));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn returns_empty_when_no_token_resolves_to_dir() {
        let root = fresh_root("no_match");
        let scope = enumerate_scope_files(&root, "rewrite Myproject/foo/myproject1/ to do X");
        assert!(scope.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn walks_real_directory_and_lists_source_files() {
        let root = fresh_root("real_dir");
        fs::create_dir_all(root.join("crates/foo/src")).unwrap();
        fs::write(root.join("crates/foo/src/lib.rs"), "// lib").unwrap();
        fs::write(root.join("crates/foo/src/notes.txt"), "skip me").unwrap();
        let scope = enumerate_scope_files(&root, "edit crates/foo/");
        assert!(scope.iter().any(|p| p.ends_with("lib.rs")));
        assert!(!scope.iter().any(|p| p.ends_with("notes.txt"))); // wrong ext
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn skips_target_node_modules_and_dot_dirs() {
        let root = fresh_root("skipped");
        fs::create_dir_all(root.join("crates/foo/target/build")).unwrap();
        fs::create_dir_all(root.join("crates/foo/node_modules/dep")).unwrap();
        fs::create_dir_all(root.join("crates/foo/.cache")).unwrap();
        fs::create_dir_all(root.join("crates/foo/src")).unwrap();
        fs::write(root.join("crates/foo/target/build/keep.rs"), "x").unwrap();
        fs::write(root.join("crates/foo/node_modules/dep/keep.rs"), "x").unwrap();
        fs::write(root.join("crates/foo/.cache/keep.rs"), "x").unwrap();
        fs::write(root.join("crates/foo/src/keep.rs"), "x").unwrap();
        let scope = enumerate_scope_files(&root, "look at crates/foo/");
        assert!(scope.iter().any(|p| p.ends_with("src/keep.rs")));
        for path in &scope {
            assert!(!path.contains("target/"), "leaked target/: {path}");
            assert!(
                !path.contains("node_modules"),
                "leaked node_modules: {path}"
            );
            assert!(!path.contains(".cache"), "leaked .cache: {path}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn does_not_follow_symlinks_so_self_loops_terminate() {
        let root = fresh_root("symlink_loop");
        fs::create_dir_all(root.join("crates/foo")).unwrap();
        fs::write(root.join("crates/foo/real.rs"), "x").unwrap();
        // Make foo/loop point back at foo — would recurse forever without the symlink guard.
        symlink(root.join("crates/foo"), root.join("crates/foo/loop")).unwrap();
        let scope =
            enumerate_scope_files_with_deadline(&root, "scan crates/foo/", Duration::from_secs(2));
        assert!(scope.iter().any(|p| p.ends_with("real.rs")));
        // The symlink target's contents must NOT appear via the loop path.
        for path in &scope {
            assert!(!path.contains("loop"), "followed symlink: {path}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn caps_recursion_depth() {
        let root = fresh_root("deep");
        // Build a chain deeper than SCOPE_WALK_MAX_DEPTH so the cap is the
        // only thing that stops the walk.
        let mut p = root.join("crates/deep");
        for i in 0..(SCOPE_WALK_MAX_DEPTH + 5) {
            p = p.join(format!("d{i}"));
        }
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("buried.rs"), "x").unwrap();
        let scope = enumerate_scope_files(&root, "trace crates/deep/");
        // `buried.rs` lives below the cap, so it must not appear.
        assert!(!scope.iter().any(|p| p.ends_with("buried.rs")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn caps_total_files_returned() {
        let root = fresh_root("many_files");
        let dir = root.join("crates/many");
        fs::create_dir_all(&dir).unwrap();
        for i in 0..(SCOPE_WALK_MAX_FILES + 50) {
            fs::write(dir.join(format!("f{i}.rs")), "x").unwrap();
        }
        let scope = enumerate_scope_files(&root, "process crates/many/");
        assert_eq!(scope.len(), SCOPE_WALK_MAX_FILES);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn deadline_zero_returns_immediately_with_empty() {
        // Even if a real directory exists, a zero deadline means the
        // foreground returns an empty Vec without waiting on the worker.
        let root = fresh_root("deadline_zero");
        fs::create_dir_all(root.join("crates/foo/src")).unwrap();
        fs::write(root.join("crates/foo/src/lib.rs"), "x").unwrap();
        let scope =
            enumerate_scope_files_with_deadline(&root, "review crates/foo/", Duration::ZERO);
        assert!(scope.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    // `scope_walk_timeout` reads a process-wide env var; this test must be
    // serialized against any other test that touches the same var.
    #[test]
    fn scope_walk_timeout_env_parsing() {
        use std::sync::Mutex;
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap();
        const VAR: &str = "NIT_SCOPE_WALK_TIMEOUT_MS";

        let prior = std::env::var(VAR).ok();

        std::env::remove_var(VAR);
        assert_eq!(
            scope_walk_timeout(),
            Duration::from_millis(SCOPE_WALK_DEFAULT_TIMEOUT_MS)
        );

        std::env::set_var(VAR, "  ");
        assert_eq!(
            scope_walk_timeout(),
            Duration::from_millis(SCOPE_WALK_DEFAULT_TIMEOUT_MS)
        );

        std::env::set_var(VAR, "0");
        assert_eq!(scope_walk_timeout(), Duration::ZERO);

        std::env::set_var(VAR, "750");
        assert_eq!(scope_walk_timeout(), Duration::from_millis(750));

        std::env::set_var(VAR, "garbage");
        assert_eq!(
            scope_walk_timeout(),
            Duration::from_millis(SCOPE_WALK_DEFAULT_TIMEOUT_MS)
        );

        match prior {
            Some(value) => std::env::set_var(VAR, value),
            None => std::env::remove_var(VAR),
        }
    }

    fn run_git(args: &[&str], cwd: &Path) -> std::process::Output {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(cwd);
        // Make sure committer/author env doesn't leak from the host.
        cmd.env("GIT_AUTHOR_NAME", "scope-test")
            .env("GIT_AUTHOR_EMAIL", "scope@test")
            .env("GIT_COMMITTER_NAME", "scope-test")
            .env("GIT_COMMITTER_EMAIL", "scope@test");
        cmd.output().expect("git command")
    }

    #[test]
    fn git_fallback_includes_uncommitted_changes_when_prompt_has_no_paths() {
        // Skip cleanly when git is missing on the build host (CI sandbox,
        // minimal images, etc.) so the test doesn't false-fail.
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("git missing — skipping");
            return;
        }
        let root = fresh_root("git_changed");

        assert!(run_git(&["init", "-q", "-b", "main"], &root)
            .status
            .success());
        // Seed the repo with a committed .rs file inside `crates/`.
        fs::create_dir_all(root.join("crates/foo/src")).unwrap();
        fs::write(root.join("crates/foo/src/lib.rs"), "// initial\n").unwrap();
        assert!(run_git(&["add", "-A"], &root).status.success());
        assert!(run_git(&["commit", "-q", "-m", "seed"], &root)
            .status
            .success());
        // Now modify it — `git diff --name-only HEAD` should report it.
        fs::write(root.join("crates/foo/src/lib.rs"), "// changed\n").unwrap();

        // Prompt deliberately has no path tokens (no `/`), so the
        // directory walk returns empty and the git fallback kicks in.
        let scope = enumerate_scope_files(&root, "fix the bug");
        assert!(
            scope.iter().any(|p| p.ends_with("crates/foo/src/lib.rs")),
            "expected git fallback to include the modified .rs file, got {scope:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn git_fallback_skips_target_and_dot_dirs() {
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("git missing — skipping");
            return;
        }
        let root = fresh_root("git_filters");
        assert!(run_git(&["init", "-q", "-b", "main"], &root)
            .status
            .success());
        // Track files in target/ and .cache/ so they show up in the
        // diff. The fallback must filter them out the same way the
        // directory walk does.
        fs::create_dir_all(root.join("crates/foo/target")).unwrap();
        fs::create_dir_all(root.join("crates/foo/.cache")).unwrap();
        fs::create_dir_all(root.join("crates/foo/src")).unwrap();
        fs::write(root.join("crates/foo/target/keep.rs"), "x").unwrap();
        fs::write(root.join("crates/foo/.cache/keep.rs"), "x").unwrap();
        fs::write(root.join("crates/foo/src/keep.rs"), "x").unwrap();
        // README is wrong-extension; must not slip through.
        fs::write(root.join("README.md"), "x").unwrap();
        // Force git to track even the normally-ignored paths.
        fs::write(root.join(".gitignore"), "").unwrap();
        assert!(run_git(&["add", "-A"], &root).status.success());
        assert!(run_git(&["commit", "-q", "-m", "seed"], &root)
            .status
            .success());
        // Touch all four files so they show in the diff.
        fs::write(root.join("crates/foo/target/keep.rs"), "y").unwrap();
        fs::write(root.join("crates/foo/.cache/keep.rs"), "y").unwrap();
        fs::write(root.join("crates/foo/src/keep.rs"), "y").unwrap();
        fs::write(root.join("README.md"), "y").unwrap();

        let scope = enumerate_scope_files(&root, "general cleanup");
        assert!(scope.iter().any(|p| p.ends_with("src/keep.rs")));
        for path in &scope {
            assert!(!path.contains("target/"), "leaked target/: {path}");
            assert!(!path.contains(".cache"), "leaked .cache: {path}");
            assert!(!path.ends_with(".md"), "wrong-ext slipped: {path}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn git_fallback_returns_empty_when_not_a_git_repo() {
        // Plain temp dir, no `git init`. The fallback must fail
        // gracefully and not panic / hang / leak the host's repo.
        let root = fresh_root("not_a_repo");
        let scope = enumerate_scope_files(&root, "do something");
        assert!(scope.is_empty(), "expected empty scope, got {scope:?}");
        let _ = fs::remove_dir_all(&root);
    }
}
