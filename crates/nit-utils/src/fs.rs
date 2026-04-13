use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

struct TempGuard<'a> {
    path: Option<&'a Path>,
}

impl Drop for TempGuard<'_> {
    fn drop(&mut self) {
        if let Some(path) = self.path {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn write_atomic<F>(path: &Path, f: F) -> io::Result<()>
where
    F: FnOnce(&mut BufWriter<File>) -> io::Result<()>,
{
    let tmp_path = path.with_extension("tmp");
    let file = File::create(&tmp_path)?;
    let mut guard = TempGuard {
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

pub fn ensure_dir(target: &Path) -> io::Result<&Path> {
    fs::create_dir_all(target)?;
    Ok(target)
}
