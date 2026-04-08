//! Atomic file writing utilities.
//!
//! Provides [`write_atomic`], which writes data through a temporary sibling file
//! and renames it into place, ensuring readers never observe a partially-written
//! target.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

/// Drop guard that removes a temporary file unless explicitly disarmed.
///
/// Used by [`write_atomic`] to ensure cleanup on failure paths without
/// requiring explicit error handling at each step.
struct TmpGuard<'a> {
    path: &'a Path,
    armed: bool,
}

impl<'a> TmpGuard<'a> {
    /// Creates a new armed guard for the given temporary file path.
    fn new(path: &'a Path) -> Self {
        Self { path, armed: true }
    }

    /// Disarms the guard so it will not remove the file on drop.
    ///
    /// Call this after the temporary file has been successfully renamed
    /// into its final location.
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

/// Writes to `path` atomically by first writing to a temporary sibling file,
/// then renaming it into place.
///
/// The caller provides a closure that receives a buffered writer and should
/// write the desired content. On success the data is flushed, `fsync`-ed, and
/// renamed over the target path in a single atomic operation (on most
/// filesystems).
///
/// # Temporary file path
///
/// The temporary file is created by replacing the target's extension with
/// `"tmp"` via [`Path::with_extension`]. This means:
///
/// - `data.json` produces `data.tmp`.
/// - `data.tar.gz` produces `data.tar.tmp` (only the final extension changes).
/// - Two targets differing only in extension (e.g. `grid.json` and `grid.rle`
///   in the same directory) will collide on the same `grid.tmp` — avoid
///   concurrent atomic writes to such pairs.
///
/// # Error handling
///
/// If the write closure, flush, sync, or rename fails, the temporary file is
/// automatically removed by a [`TmpGuard`] drop guard. This prevents leftover
/// `.tmp` files from accumulating on repeated failures.
///
/// # Errors
///
/// Returns [`io::Error`] if the temporary file cannot be created, the write
/// closure fails, flushing or syncing fails, or the final rename fails.
pub fn write_atomic<F>(path: &Path, write_fn: F) -> io::Result<()>
where
    F: FnOnce(&mut BufWriter<File>) -> io::Result<()>,
{
    let tmp_path = path.with_extension("tmp");
    let file = File::create(&tmp_path)?;
    let mut guard = TmpGuard::new(&tmp_path);
    let mut writer = BufWriter::new(file);
    write_fn(&mut writer)?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    fs::rename(&tmp_path, path)?;
    guard.disarm();
    Ok(())
}
