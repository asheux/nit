//! Atomic NDJSON file writer.
//!
//! `AtomicNdjsonWriter` streams JSON values one-per-line into a `.tmp`
//! sibling, fsyncs on `finish()`, then renames into the final path so
//! readers never observe a half-written file.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

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

    pub fn path(&self) -> &Path {
        &self.final_path
    }
}
