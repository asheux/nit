use std::fs;
use std::path::{Path, PathBuf};
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
pub(crate) fn enumerate_scope_files(workspace_root: &Path, prompt: &str) -> Vec<String> {
    enumerate_scope_files_with_deadline(workspace_root, prompt, scope_walk_timeout())
}

pub(crate) fn enumerate_scope_files_with_deadline(
    workspace_root: &Path,
    prompt: &str,
    deadline: Duration,
) -> Vec<String> {
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
        return Vec::new();
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
}
