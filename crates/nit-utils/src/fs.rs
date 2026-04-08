//! Atomic file-writing utilities.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

struct TmpGuard<'a> {
    path: &'a Path,
    armed: bool,
}

impl<'a> TmpGuard<'a> {
    fn new(path: &'a Path) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TmpGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(self.path);
        }
    }
}

/// Writes to `path` atomically via a temporary sibling file (`*.tmp`).
/// The closure receives a buffered writer; on success the data is flushed,
/// synced, and renamed into place. The temp file is cleaned up on failure.
pub fn write_atomic<F>(path: &Path, write_fn: F) -> io::Result<()>
where
    F: FnOnce(&mut BufWriter<File>) -> io::Result<()>,
{
    let tmp_path = path.with_extension("tmp");
    let file = File::create(&tmp_path)?;
    let mut guard = TmpGuard::new(&tmp_path);
    let mut writer = BufWriter::new(file);
    write_fn(&mut writer)?;
    flush_and_persist(&mut writer, &tmp_path, path)?;
    guard.disarm();
    Ok(())
}

fn flush_and_persist(writer: &mut BufWriter<File>, tmp: &Path, dest: &Path) -> io::Result<()> {
    writer.flush()?;
    writer.get_ref().sync_all()?;
    fs::rename(tmp, dest)
}
