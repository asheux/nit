//! Serializable match-history records and buffered NDJSON writer.
//!
//! Records are written to a temporary file first and atomically renamed on
//! completion to avoid partial or corrupt output.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
    /// Falls back to `score_idx` length when `rounds` is zero.
    pub fn resolved_rounds(&self) -> u32 {
        if self.rounds > 0 {
            return self.rounds;
        }
        self.score_idx.len().min(u32::MAX as usize) as u32
    }
}

/// Buffered NDJSON writer for [`MatchHistory`] records.
pub struct HistoryWriter {
    inner: crate::ndjson::AtomicNdjsonWriter,
}

impl HistoryWriter {
    pub fn new(final_path: PathBuf) -> io::Result<Self> {
        Ok(Self {
            inner: crate::ndjson::AtomicNdjsonWriter::create(final_path)?,
        })
    }

    pub fn write(&mut self, record: &MatchHistory) -> io::Result<()> {
        self.inner.append(record)
    }

    pub fn finish(self) -> io::Result<PathBuf> {
        self.inner.finish()
    }

    pub fn final_path(&self) -> &Path {
        self.inner.final_path()
    }
}

#[cfg(test)]
#[path = "test_modules/history_log.rs"]
mod tests;
