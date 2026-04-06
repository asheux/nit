//! Structured NDJSON event logging for tournament execution.
//!
//! This module defines [`GameEvent`], the set of structured events emitted
//! during a tournament run, and [`EventWriter`], a buffered writer that
//! serialises those events as newline-delimited JSON (NDJSON) to disk.
//!
//! Events are written to a temporary file first and atomically renamed on
//! completion to avoid partial or corrupt output.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

// ── Event log configuration ───────────────────────────────────────────────

/// Configuration for the structured event log emitted during a tournament run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventLogConfig {
    /// Whether event logging is enabled at all.
    pub enabled: bool,

    /// Whether individual round events are included (can be very verbose).
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

// ── Game event enum ───────────────────────────────────────────────────────

/// A structured event emitted during tournament execution.
///
/// Serialized as newline-delimited JSON (NDJSON) with a `"event"` tag field.
/// Each variant captures a distinct phase of the tournament lifecycle.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum GameEvent {
    /// Emitted once at the very start of a tournament run.
    TournamentStart {
        /// RFC 3339 timestamp when the tournament began.
        timestamp: String,
        /// Total number of matches that will be played.
        total_matches: usize,
        /// Number of rounds per match.
        rounds: u32,
    },

    /// Emitted when an individual match is about to begin.
    MatchStart {
        /// RFC 3339 timestamp when the match started.
        timestamp: String,
        /// Unique identifier for this match within the tournament.
        match_id: usize,
        /// Zero-based position in the schedule.
        match_index: usize,
        /// Total number of matches in the tournament.
        total_matches: usize,
        /// Strategy identifier for player A.
        a: String,
        /// Strategy identifier for player B.
        b: String,
        /// Repetition number for this pairing.
        repetition: u32,
    },

    /// Emitted after each round within a match (only when `include_rounds` is set).
    Round {
        /// RFC 3339 timestamp for this round.
        timestamp: String,
        /// Parent match identifier.
        match_id: usize,
        /// Parent match schedule index.
        match_index: usize,
        /// One-based round number within the match.
        round: u32,
        /// Action character chosen by player A (`'C'` or `'D'`).
        a_action: char,
        /// Action character chosen by player B (`'C'` or `'D'`).
        b_action: char,
        /// Whether player A's Turing machine has halted (defaults to `true`).
        #[serde(default = "default_true")]
        a_halted: bool,
        /// Whether player B's Turing machine has halted (defaults to `true`).
        #[serde(default = "default_true")]
        b_halted: bool,
        /// Payoff awarded to player A this round.
        a_payoff: i32,
        /// Payoff awarded to player B this round.
        b_payoff: i32,
    },

    /// Emitted when a match finishes, carrying the final cumulative scores.
    MatchEnd {
        /// RFC 3339 timestamp when the match ended.
        timestamp: String,
        /// Match identifier (same as the corresponding `MatchStart`).
        match_id: usize,
        /// Match schedule index.
        match_index: usize,
        /// Cumulative payoff for player A across all rounds.
        a_total: i64,
        /// Cumulative payoff for player B across all rounds.
        b_total: i64,
    },

    /// Emitted when a strategy fails during construction or execution.
    StrategyError {
        /// RFC 3339 timestamp of the error.
        timestamp: String,
        /// Identifier of the strategy that failed.
        strategy_id: String,
        /// Human-readable error description.
        error: String,
    },

    /// Emitted once after the last match completes.
    TournamentEnd {
        /// RFC 3339 timestamp when the tournament finished.
        timestamp: String,
    },
}

/// Serde default function -- returns `true`, used for the `a_halted` / `b_halted`
/// fields so that non-TM matches deserialise correctly.
fn default_true() -> bool {
    true
}

// ── Event writer ──────────────────────────────────────────────────────────

/// Buffered NDJSON writer for [`GameEvent`]s.
///
/// Writes to a temporary file first, then atomically renames to the final
/// path on [`finish`](Self::finish) to prevent partial/corrupt output files.
pub struct EventWriter {
    /// Buffered handle to the temporary output file.
    writer: BufWriter<File>,
    /// Path of the temporary file being written.
    tmp_path: PathBuf,
    /// Destination path after the atomic rename.
    final_path: PathBuf,
    /// Whether per-round events should be recorded.
    include_rounds: bool,
}

impl EventWriter {
    /// Creates a new writer that will produce `final_path` on completion.
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

    /// Returns whether round-level events should be recorded.
    pub fn include_rounds(&self) -> bool {
        self.include_rounds
    }

    /// Serializes and appends a single event as one NDJSON line.
    pub fn write(&mut self, event: &GameEvent) -> io::Result<()> {
        serde_json::to_writer(&mut self.writer, event).map_err(io::Error::other)?;
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

    /// Returns the current UTC timestamp formatted as RFC 3339.
    pub fn timestamp() -> String {
        OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "unknown-time".into())
    }

    /// Generates a default file-safe name using the given prefix and a timestamp.
    pub fn default_name(prefix: &str) -> String {
        let timestamp = Self::timestamp().replace(':', "-");
        format!("{prefix}__{timestamp}")
    }

    /// Returns the destination path that the log will be written to.
    pub fn final_path(&self) -> &Path {
        &self.final_path
    }
}
