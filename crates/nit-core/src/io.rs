use crate::buffer::Buffer;
use std::fs::File;
use std::io::{Read, Write};
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
    let mut file = File::open(path)?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    Ok(buf)
}

pub fn save_buffer(buffer: &Buffer) -> Result<()> {
    let path = buffer.path().ok_or(IoError::MissingPath)?;
    let content = buffer.content_as_string();
    nit_utils::fs::write_atomic(path, |w| w.write_all(content.as_bytes()))?;
    Ok(())
}
