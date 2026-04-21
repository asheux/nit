use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

// `path.with_extension("tmp")` would turn `foo.json` into `foo.tmp`, so two
// concurrent writes that differ only by extension would race on the same
// sibling. A process-unique suffix avoids the collision without depending on
// clock resolution.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmp_sibling(path: &Path) -> PathBuf {
    let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".tmp.{}.{counter}", std::process::id()));
    PathBuf::from(name)
}

struct TempGuard<'a> {
    path: &'a Path,
    armed: bool,
}

impl Drop for TempGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(self.path);
        }
    }
}

pub fn write_atomic<F>(path: &Path, f: F) -> io::Result<()>
where
    F: FnOnce(&mut BufWriter<File>) -> io::Result<()>,
{
    let tmp_path = tmp_sibling(path);
    let file = File::create(&tmp_path)?;
    let mut guard = TempGuard {
        path: &tmp_path,
        armed: true,
    };
    let mut writer = BufWriter::new(file);

    f(&mut writer)?;

    writer.flush()?;
    writer.get_ref().sync_all()?;
    fs::rename(&tmp_path, path)?;

    guard.armed = false;
    Ok(())
}

pub fn ensure_dir(target: &Path) -> io::Result<&Path> {
    fs::create_dir_all(target)?;
    Ok(target)
}
