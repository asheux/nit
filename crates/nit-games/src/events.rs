//! Structured NDJSON event logging for tournament execution.
//!
//! Events are written to a temporary file first and atomically renamed on
//! completion to avoid partial or corrupt output.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventLogConfig {
    pub enabled: bool,
    /// Including per-round events can be very verbose.
    pub include_rounds: bool,
}

impl Default for EventLogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            include_rounds: false,
        }
    }
}

/// Structured event emitted during tournament execution, serialized as NDJSON.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum GameEvent {
    TournamentStart {
        timestamp: String,
        total_matches: usize,
        rounds: u32,
    },
    MatchStart {
        timestamp: String,
        match_id: usize,
        match_index: usize,
        total_matches: usize,
        a: String,
        b: String,
        repetition: u32,
    },
    /// Only emitted when `include_rounds` is enabled.
    Round {
        timestamp: String,
        match_id: usize,
        match_index: usize,
        round: u32,
        a_action: char,
        b_action: char,
        /// Defaults to `true` so non-TM matches deserialize correctly.
        #[serde(default = "default_true")]
        a_halted: bool,
        #[serde(default = "default_true")]
        b_halted: bool,
        a_payoff: i32,
        b_payoff: i32,
    },
    MatchEnd {
        timestamp: String,
        match_id: usize,
        match_index: usize,
        a_total: i64,
        b_total: i64,
    },
    StrategyError {
        timestamp: String,
        strategy_id: String,
        error: String,
    },
    TournamentEnd {
        timestamp: String,
    },
}

fn default_true() -> bool {
    true
}

/// Buffered NDJSON writer that writes to a temp file and atomically renames
/// on completion to prevent partial output.
pub struct EventWriter {
    writer: BufWriter<File>,
    tmp_path: PathBuf,
    final_path: PathBuf,
    include_rounds: bool,
}

impl EventWriter {
    pub fn new(final_path: PathBuf, include_rounds: bool) -> io::Result<Self> {
        let tmp_path = final_path.with_extension("ndjson.tmp");
        let file = File::create(&tmp_path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            tmp_path,
            final_path,
            include_rounds,
        })
    }

    pub fn include_rounds(&self) -> bool {
        self.include_rounds
    }

    pub fn write(&mut self, event: &GameEvent) -> io::Result<()> {
        serde_json::to_writer(&mut self.writer, event).map_err(io::Error::other)?;
        self.writer.write_all(b"\n")
    }

    /// Flushes, syncs, and atomically renames the temp file to the final path.
    pub fn finish(mut self) -> io::Result<PathBuf> {
        self.writer.flush()?;
        self.writer.get_ref().sync_all()?;
        fs::rename(&self.tmp_path, &self.final_path)?;
        Ok(self.final_path)
    }

    pub fn timestamp() -> String {
        OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "unknown-time".into())
    }

    pub fn default_name(prefix: &str) -> String {
        let ts = Self::timestamp().replace(':', "-");
        format!("{prefix}__{ts}")
    }

    pub fn final_path(&self) -> &Path {
        &self.final_path
    }
}
