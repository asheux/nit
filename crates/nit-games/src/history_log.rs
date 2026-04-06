// std
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

// external
use serde::{Deserialize, Serialize};

// crate
use crate::fast_eval::CycleMetadata;
use crate::output::TmDerivedMetrics;

/// Serializable record of a single match result, including scores,
/// outcome indices, and optional cycle / Turing machine metrics.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchHistory {
    /// Unique identifier for this match within the tournament.
    #[serde(default)]
    pub match_id: usize,
    /// Zero-based position of this match in the schedule.
    #[serde(default)]
    pub match_index: usize,
    /// Total number of matches in the tournament.
    #[serde(default)]
    pub total_matches: usize,
    /// Strategy identifier for player A.
    pub a: String,
    /// Strategy identifier for player B.
    pub b: String,
    /// Repetition number when the same pairing is played multiple times.
    #[serde(default)]
    pub repetition: u32,
    /// Number of rounds played in this match.
    #[serde(default)]
    pub rounds: u32,
    /// Encoded outcome-index string (one digit per round). Legacy alias: `"outcomes"`.
    #[serde(default, alias = "outcomes")]
    pub score_idx: String,
    /// Cumulative score for player A.
    #[serde(default)]
    pub a_score: i64,
    /// Cumulative score for player B.
    #[serde(default)]
    pub b_score: i64,
    /// Cycle metadata when the match outcome was determined by fast-eval cycle detection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle: Option<CycleMetadata>,
    /// Turing machine derived metrics for player A's strategy, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a_tm_metrics: Option<TmDerivedMetrics>,
    /// Turing machine derived metrics for player B's strategy, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub b_tm_metrics: Option<TmDerivedMetrics>,
}

impl MatchHistory {
    /// Returns the effective number of rounds, falling back to the length
    /// of the `score_idx` string when the `rounds` field is zero.
    pub fn resolved_rounds(&self) -> u32 {
        if self.rounds > 0 {
            self.rounds
        } else {
            self.score_idx.len().min(u32::MAX as usize) as u32
        }
    }
}

/// Buffered NDJSON writer for [`MatchHistory`] records.
///
/// Writes to a temporary file and atomically renames on
/// [`finish`](Self::finish) to avoid partial output.
pub struct HistoryWriter {
    writer: BufWriter<File>,
    tmp_path: PathBuf,
    final_path: PathBuf,
}

impl HistoryWriter {
    /// Creates a new writer targeting the given `final_path`.
    pub fn new(final_path: PathBuf) -> io::Result<Self> {
        let tmp_path = final_path.with_extension("ndjson.tmp");
        let file = File::create(&tmp_path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            tmp_path,
            final_path,
        })
    }

    /// Serializes and appends a single match record as one NDJSON line.
    pub fn write(&mut self, record: &MatchHistory) -> io::Result<()> {
        serde_json::to_writer(&mut self.writer, record).map_err(io::Error::other)?;
        self.writer.write_all(b"\n")?;
        Ok(())
    }

    /// Flushes, syncs, and atomically renames the temporary file to the final path.
    pub fn finish(mut self) -> io::Result<PathBuf> {
        self.writer.flush()?;
        self.writer.get_ref().sync_all()?;
        fs::rename(&self.tmp_path, &self.final_path)?;
        Ok(self.final_path)
    }

    /// Returns the destination path that the log will be written to.
    pub fn final_path(&self) -> &Path {
        &self.final_path
    }
}

#[cfg(test)]
#[path = "test_modules/history_log.rs"]
mod tests;
