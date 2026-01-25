use crate::buffer::Buffer;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
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

fn temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_else(|| "untitled".into());
    let mut tmp_name = String::from(".");
    tmp_name.push_str(&file_name);
    tmp_name.push_str(".nit.tmp");
    path.with_file_name(tmp_name)
}

pub fn save_buffer(buffer: &Buffer) -> Result<()> {
    let path = buffer.path().ok_or(IoError::MissingPath)?;
    let content = buffer.content_as_string();
    let tmp = temp_path(path);
    {
        let mut file = File::create(&tmp)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}
