//! Atomic save/load of the on-disk index at
//! `<workspace>/.nit/memory/index.json`. Tolerant load: any failure
//! (missing dir, malformed bytes) returns `MissionMemoryIndex::default()`
//! so callers can build from scratch without branching.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::MissionMemoryIndex;

pub(super) fn index_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(".nit")
        .join("memory")
        .join("index.json")
}

pub fn save_index(workspace_root: &Path, index: &MissionMemoryIndex) -> io::Result<()> {
    let path = index_path(workspace_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(index).map_err(io::Error::other)?;
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Tolerant — returns `Default` on missing or corrupt file.
pub fn load_index(workspace_root: &Path) -> MissionMemoryIndex {
    let path = index_path(workspace_root);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(_) => return MissionMemoryIndex::default(),
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}
