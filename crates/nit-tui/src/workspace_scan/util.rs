use std::path::Path;
use std::time::UNIX_EPOCH;

use nit_core::AppState;

use crate::file_watcher::is_excluded_directory;

pub(super) fn file_mtime_ms(path: &Path) -> Option<u64> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let since_epoch = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(since_epoch.as_millis() as u64)
}

/// True when `path` lives under `state.workspace_root` and none of its
/// path components are excluded (hidden, gitignored, or in `extra_dirs`).
/// Paths outside the workspace (e.g. `/tmp`) are rejected so the file
/// watcher doesn't pull in unrelated changes.
///
/// `extra_dirs` is checked AFTER the gitignore-aware
/// `is_excluded_directory` pass as belt-and-braces — a future caller
/// might pass a `gitignored_dirs` list with those entries scrubbed.
pub(super) fn is_within_workspace_scope(
    state: &AppState,
    path: &Path,
    extra_dirs: &[&str],
) -> bool {
    let Ok(relative) = path.strip_prefix(&state.workspace_root) else {
        return false;
    };
    for component in relative.components() {
        let Some(segment) = component.as_os_str().to_str() else {
            continue;
        };
        if is_excluded_directory(segment, &state.gitignored_dirs) {
            return false;
        }
    }
    for component in relative.components() {
        if let Some(segment) = component.as_os_str().to_str() {
            if extra_dirs.contains(&segment) {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
pub fn forget_report(state: &mut AppState, path: &Path) {
    let workspace_root = state.workspace_root.clone();
    if state.genome_reports.remove(path).is_some() {
        nit_core::agent_bus::delete_genome_report(&workspace_root, path);
    }
}
