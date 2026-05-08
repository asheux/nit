//! Serializable match-history records and buffered NDJSON writer.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fast_eval::CycleMetadata;
use crate::output::TmDerivedMetrics;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchHistory {
    #[serde(default)]
    pub match_id: usize,
    #[serde(default)]
    pub match_index: usize,
    #[serde(default)]
    pub total_matches: usize,
    pub a: String,
    pub b: String,
    #[serde(default)]
    pub repetition: u32,
    #[serde(default)]
    pub rounds: u32,
    /// One digit per round; legacy logs used `"outcomes"` as the field name.
    #[serde(default, alias = "outcomes")]
    pub score_idx: String,
    #[serde(default)]
    pub a_score: i64,
    #[serde(default)]
    pub b_score: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle: Option<CycleMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a_tm_metrics: Option<TmDerivedMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub b_tm_metrics: Option<TmDerivedMetrics>,
}

impl MatchHistory {
    /// Falls back to the per-round digit count in `score_idx` when
    /// `rounds` is unset (legacy logs predating the explicit field).
    pub fn resolved_rounds(&self) -> u32 {
        if self.rounds > 0 {
            return self.rounds;
        }
        self.score_idx.len().min(u32::MAX as usize) as u32
    }
}

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
        self.inner.path()
    }
}
