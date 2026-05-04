use std::fs;
use std::path::{Path, PathBuf};

use nit_core::{io as core_io, Buffer};
use nit_utils::hashing::stable_hash_bytes;
use nit_utils::paths;

pub(crate) fn load_notes(workspace_root: &Path) -> Buffer {
    let Some(notes_path) = hashed_state_path(workspace_root, "notes", "md") else {
        return Buffer::empty("notes", None);
    };
    let saved = notes_path
        .exists()
        .then(|| core_io::load_to_string(&notes_path).ok())
        .flatten();
    match saved {
        Some(content) => Buffer::from_str("notes", &content, Some(notes_path)),
        None => Buffer::empty("notes", Some(notes_path)),
    }
}

/// Build a hashed path under the nit state directory: `<state>/<subdir>/<hash>.<ext>`.
fn hashed_state_path(workspace_root: &Path, subdir: &str, ext: &str) -> Option<PathBuf> {
    let base = paths::state_dir().or_else(paths::data_dir)?;
    let dir = base.join(subdir);
    let _ = fs::create_dir_all(&dir);
    let hash = stable_hash_bytes(workspace_root.to_string_lossy().as_bytes());
    Some(dir.join(format!("{hash:016x}.{ext}")))
}

pub(crate) fn export_legacy_notes_snapshot(
    workspace_root: &Path,
    buffer: &Buffer,
) -> Option<PathBuf> {
    let notes_text = buffer.content_as_string();
    if notes_text.trim().is_empty() {
        return None;
    }
    let snapshot_path = workspace_root.join(".nit/legacy_notes.md");
    if snapshot_path.exists() {
        return Some(snapshot_path);
    }
    if let Some(parent_dir) = snapshot_path.parent() {
        let _ = fs::create_dir_all(parent_dir);
    }
    fs::write(&snapshot_path, notes_text)
        .ok()
        .map(|()| snapshot_path)
}
