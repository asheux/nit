//! Filesystem utilities: atomic writes and directory creation.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

/// Removes the temporary file on drop unless the inner path is cleared.
struct TempFileGuard<'a> {
    path: Option<&'a Path>,
}

impl Drop for TempFileGuard<'_> {
    fn drop(&mut self) {
        if let Some(path) = self.path {
            let _ = fs::remove_file(path);
        }
    }
}

/// Writes via a temporary sibling file (`*.tmp`) that is flushed, synced, and
/// renamed into place. The temp file is cleaned up on failure or panic.
pub fn write_atomic<F>(path: &Path, f: F) -> io::Result<()>
where
    F: FnOnce(&mut BufWriter<File>) -> io::Result<()>,
{
    let tmp_path = path.with_extension("tmp");
    let file = File::create(&tmp_path)?;
    let mut guard = TempFileGuard {
        path: Some(&tmp_path),
    };
    let mut writer = BufWriter::new(file);

    f(&mut writer)?;

    writer.flush()?;
    writer.get_ref().sync_all()?;
    fs::rename(&tmp_path, path)?;

    guard.path = None;
    Ok(())
}

/// Creates `target` and all missing ancestors, returning `target` on success.
pub fn ensure_dir(target: &Path) -> io::Result<&Path> {
    fs::create_dir_all(target)?;
    Ok(target)
}
