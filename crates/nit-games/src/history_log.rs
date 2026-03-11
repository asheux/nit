use serde::{Deserialize, Serialize};

use crate::fast_eval::CycleMetadata;
use crate::output::TmDerivedMetrics;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

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
    pub fn resolved_rounds(&self) -> u32 {
        if self.rounds > 0 {
            self.rounds
        } else {
            self.score_idx.len().min(u32::MAX as usize) as u32
        }
    }
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
        serde_json::to_writer(&mut self.writer, record).map_err(io::Error::other)?;
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

#[cfg(test)]
mod tests {
    use super::MatchHistory;

    #[test]
    fn match_history_serializes_compact_payload() {
        let record = MatchHistory {
            match_id: 7,
            match_index: 8,
            total_matches: 12,
            a: "fsm_a".into(),
            b: "fsm_b".into(),
            repetition: 1,
            rounds: 4,
            score_idx: "0123".into(),
            a_score: -6,
            b_score: -2,
            cycle: None,
            a_tm_metrics: None,
            b_tm_metrics: None,
        };

        let json = serde_json::to_string(&record).expect("serialize compact history");
        assert!(json.contains("\"score_idx\":\"0123\""));
        assert!(json.contains("\"a\":\"fsm_a\""));
        assert!(json.contains("\"b\":\"fsm_b\""));
        assert!(!json.contains("a_moves"));
        assert!(!json.contains("b_moves"));
        assert!(!json.contains("timestamp"));
        assert!(!json.contains("event"));
    }
}
