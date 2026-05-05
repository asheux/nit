use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub(super) const SCOPE_WALK_MAX_FILES: usize = 100;

pub(super) const SCOPE_WALK_MAX_DEPTH: usize = 12;

pub(super) const SCOPE_WALK_DEFAULT_TIMEOUT_MS: u64 = 200;

const SCOPE_WALK_SKIP_DIRS: &[&str] = &["target", "node_modules"];

fn is_source_extension(ext: &str) -> bool {
    matches!(
        ext,
        "rs" | "toml"
            | "ts"
            | "js"
            | "tsx"
            | "jsx"
            | "py"
            | "go"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "lua"
            | "rb"
            | "conf"
            | "md"
    )
}

// `Path::extension()` returns `None` for `.zshrc` (the whole basename is the
// file name with no separate extension), so the leading-dot pattern is the
// only reliable way to surface dotfile-style sources.
fn is_source_filename(name: &str) -> bool {
    matches!(
        name,
        ".zshrc"
            | ".zshenv"
            | ".zprofile"
            | ".bashrc"
            | ".bash_profile"
            | ".profile"
            | ".tmux.conf"
            | ".vimrc"
            | ".gvimrc"
            | "Makefile"
            | "Dockerfile"
    )
}

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

/// Extract directory tokens from the operator prompt and enumerate their
/// source files. Result is alphabetically sorted and capped at
/// `SCOPE_WALK_MAX_FILES`.
///
/// The walk runs on a background thread; the foreground waits up to
/// `scope_walk_timeout()` (default 200 ms; override via
/// `NIT_SCOPE_WALK_TIMEOUT_MS`). On slow disks or pathological trees the
/// foreground returns an empty `Vec` so the UI never freezes — the walker
/// continues in the background and its result is discarded.
///
/// When the prompt contains no path tokens (no token with `/`), the walk
/// falls back to `git diff --name-only` against the merge-base with `main`
/// (or `master`) so the verifier can scope to in-progress edits instead of
/// running `--workspace`. Empty when not a git repo or git is unavailable.
pub(crate) fn enumerate_scope_files(workspace_root: &Path, prompt: &str) -> Vec<String> {
    enumerate_scope_files_with_deadline(workspace_root, prompt, scope_walk_timeout())
}

pub(crate) fn enumerate_scope_files_with_deadline(
    workspace_root: &Path,
    prompt: &str,
    deadline: Duration,
) -> Vec<String> {
    // `recv_timeout(Duration::ZERO)` races with the worker on fast machines
    // (the walker can deliver before the timeout fires), so short-circuit
    // before spawning when callers explicitly request "skip the walk".
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
        // Spawn failure (fd exhaustion) → no-scope rather than block the UI
        // on a synchronous walk.
        return Vec::new();
    }

    rx.recv_timeout(deadline).unwrap_or_default()
}

fn enumerate_scope_files_blocking(workspace_root: &Path, prompt: &str) -> Vec<String> {
    let dirs = collect_prompt_dirs(workspace_root, prompt);
    if dirs.is_empty() {
        // No usable path tokens. Fall back to operator's working-tree edits
        // so the verifier still runs scoped commands instead of collapsing
        // to `--workspace --all-features`.
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

fn collect_prompt_dirs(workspace_root: &Path, prompt: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for token in prompt.split_whitespace() {
        let token = token.trim_matches(|c: char| c == ',' || c == '.' || c == '"' || c == '\'');
        if token.is_empty() || !token.contains('/') {
            continue;
        }
        let candidate = workspace_root.join(token);
        if candidate.is_dir() {
            dirs.push(candidate);
        }
    }
    dirs
}

// Last-resort scope source: ask git which files changed in the current
// branch, starting from the merge-base with `main`/`master`. Filtered to
// the walk's accepted extensions, capped at `SCOPE_WALK_MAX_FILES`. Empty
// on any failure (not a git repo, git missing, no merge-base, garbled
// output) — caller treats empty as "no scope" and falls back to unscoped
// commands.
fn git_changed_scope_files(workspace_root: &Path) -> Vec<String> {
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
        .filter(|line| has_source_basename(line))
        .filter(|line| !line_is_in_skipped_dir(line))
        .filter(|line| workspace_root.join(line).exists())
        .map(String::from)
        .collect();
    files.sort();
    files.dedup();
    files.truncate(SCOPE_WALK_MAX_FILES);
    files
}

fn has_source_basename(line: &str) -> bool {
    let path = Path::new(line);
    let ext_match = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(is_source_extension);
    let filename_match = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(is_source_filename);
    ext_match || filename_match
}

// Skip files in directories the walk skips so the two sources stay
// consistent. Allow leaf files starting with `.` (dotfiles like .zshrc) —
// only intermediate dot-DIRS are skipped.
fn line_is_in_skipped_dir(line: &str) -> bool {
    let parts: Vec<&str> = line.split('/').collect();
    let dir_components: &[&str] = if parts.len() > 1 {
        &parts[..parts.len() - 1]
    } else {
        &[]
    };
    dir_components
        .iter()
        .any(|component| component.starts_with('.') || SCOPE_WALK_SKIP_DIRS.contains(component))
}

// Walk ancestors looking for `merge-base HEAD <branch>` against `main`,
// then `master`. `None` when neither exists; caller falls back to `HEAD`
// so we still catch uncommitted edits.
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
    if depth >= SCOPE_WALK_MAX_DEPTH || out.len() >= SCOPE_WALK_MAX_FILES {
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
        // self-loops or upward-pointing links) shows up as a symlink, not as
        // its target. This is the cycle guard — cheap and traversal-free.
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
        } else if meta.is_file() && has_source_basename_path(&path) {
            if let Ok(rel) = path.strip_prefix(workspace_root) {
                out.push(rel.display().to_string());
            }
        }
    }
}

fn has_source_basename_path(path: &Path) -> bool {
    has_source_basename(path.to_str().unwrap_or(""))
}

pub(super) fn scope_walk_timeout() -> Duration {
    let default = Duration::from_millis(SCOPE_WALK_DEFAULT_TIMEOUT_MS);
    let Ok(raw) = std::env::var("NIT_SCOPE_WALK_TIMEOUT_MS") else {
        return default;
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return default;
    }
    match raw.parse::<u64>() {
        // `0` = "skip the walk entirely". Worker thread is still spawned but
        // the foreground returns Vec::new() right away.
        Ok(0) => Duration::ZERO,
        Ok(ms) => Duration::from_millis(ms),
        Err(_) => default,
    }
}
