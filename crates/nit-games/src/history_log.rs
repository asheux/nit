use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchHistory {
    pub event: String,
    pub timestamp: String,
    pub match_index: usize,
    pub total_matches: usize,
    pub a: String,
    pub b: String,
    pub repetition: u32,
    pub rounds: u32,
    pub outcomes: String,
}

pub struct HistoryWriter {
    writer: BufWriter<File>,
    tmp_path: PathBuf,
    final_path: PathBuf,
}

impl HistoryWriter {
    pub fn new(final_path: PathBuf) -> io::Result<Self> {
        let tmp_path = final_path.with_extension("ndjson.tmp");
        let file = File::create(&tmp_path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            tmp_path,
            final_path,
        })
    }

    pub fn write(&mut self, record: &MatchHistory) -> io::Result<()> {
        serde_json::to_writer(&mut self.writer, record)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        self.writer.write_all(b"\n")?;
        Ok(())
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
