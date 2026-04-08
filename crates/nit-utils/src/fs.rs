//! Atomic file-writing utilities.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

/// Removes the temporary file on drop unless disarmed.
struct TempFileGuard<'a> {
    path: Option<&'a Path>,
}

impl<'a> TempFileGuard<'a> {
    fn new(path: &'a Path) -> Self {
        Self { path: Some(path) }
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for TempFileGuard<'_> {
    fn drop(&mut self) {
        if let Some(path) = self.path {
            let _ = fs::remove_file(path);
        }
    }
}

/// Writes via a temporary sibling file (`*.tmp`) that is flushed, synced, and
/// renamed into place. The temp file is cleaned up on failure.
pub fn write_atomic<F>(path: &Path, f: F) -> io::Result<()>
where
    F: FnOnce(&mut BufWriter<File>) -> io::Result<()>,
{
    let tmp_path = path.with_extension("tmp");
    let file = File::create(&tmp_path)?;
    let mut guard = TempFileGuard::new(&tmp_path);
    let mut writer = BufWriter::new(file);

    f(&mut writer)?;

    writer.flush()?;
    writer.get_ref().sync_all()?;
    fs::rename(&tmp_path, path)?;

    guard.disarm();
    Ok(())
}
