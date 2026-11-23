use crate::buffer::Buffer;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IoError {
    #[error("buffer has no path")]
    MissingPath,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, IoError>;

pub fn load_to_string(path: &Path) -> Result<String> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "expected a regular file",
        )
        .into());
    }
    let mut file = File::open(path)?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    Ok(buf)
}

pub fn save_buffer(buffer: &Buffer) -> Result<()> {
    let path = buffer.path().ok_or(IoError::MissingPath)?;
    // Opening a symlink reads its target. Saving should update the same file.
    let target;
    let path = if path.is_symlink() {
        target = path.canonicalize()?;
        &target
    } else {
        path
    };
    nit_utils::fs::write_atomic(path, |w| buffer.rope_ref().write_to(w))?;
    Ok(())
}
