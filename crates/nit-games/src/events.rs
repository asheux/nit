//! Structured NDJSON event logging for tournament execution.

use std::io;
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
    Round {
        timestamp: String,
        match_id: usize,
        match_index: usize,
        round: u32,
        a_action: char,
        b_action: char,
        /// Defaults to `true` for backward compat with non-TM match records.
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

/// Writes NDJSON event records with atomic rename on completion.
pub struct EventWriter {
    inner: crate::ndjson::AtomicNdjsonWriter,
    include_rounds: bool,
}

impl EventWriter {
    pub fn new(final_path: PathBuf, include_rounds: bool) -> io::Result<Self> {
        Ok(Self {
            inner: crate::ndjson::AtomicNdjsonWriter::create(final_path)?,
            include_rounds,
        })
    }

    pub fn include_rounds(&self) -> bool {
        self.include_rounds
    }

    pub fn write(&mut self, event: &GameEvent) -> io::Result<()> {
        self.inner.append(event)
    }

    pub fn finish(self) -> io::Result<PathBuf> {
        self.inner.finish()
    }

    pub fn timestamp() -> String {
        OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "unknown-time".into())
    }

    pub fn default_name(prefix: &str) -> String {
        format!("{prefix}__{}", Self::timestamp().replace(':', "-"))
    }

    pub fn final_path(&self) -> &Path {
        self.inner.final_path()
    }
}
