//! Atomic NDJSON file writer used by event and history logging.
//!
//! Writes to a `.ndjson.tmp` sidecar; [`finish`](AtomicNdjsonWriter::finish)
//! atomically renames to the final path so readers never see partial output.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Buffered NDJSON writer with atomic-rename-on-finish semantics.
pub(crate) struct AtomicNdjsonWriter {
    writer: BufWriter<File>,
    tmp_path: PathBuf,
    final_path: PathBuf,
}

impl AtomicNdjsonWriter {
    pub fn create(final_path: PathBuf) -> io::Result<Self> {
        let tmp_path = final_path.with_extension("ndjson.tmp");
        let file = File::create(&tmp_path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            tmp_path,
            final_path,
        })
    }

    pub fn append<T: Serialize>(&mut self, value: &T) -> io::Result<()> {
        serde_json::to_writer(&mut self.writer, value).map_err(io::Error::other)?;
        self.writer.write_all(b"\n")
    }

    pub fn finish(mut self) -> io::Result<PathBuf> {
        self.writer.flush()?;
        self.writer.get_ref().sync_all()?;
        fs::rename(&self.tmp_path, &self.final_path)?;
        Ok(self.final_path)
    }

    pub fn final_path(&self) -> &Path {
        &self.final_path
    }
}
