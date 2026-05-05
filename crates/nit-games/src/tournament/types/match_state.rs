//! Per-match state: scheduled matchup, in-flight session, round results.

use crate::game::Action;
use crate::history::History;
use crate::strategy::{Strategy, TmRunStats};
use nit_utils::rng::SplitMix64;

/// Outcome of a single completed match: strategy indices, raw and adjusted scores.
///
/// The `adjusted_total` fields incorporate complexity-cost penalties when enabled;
/// otherwise they equal the raw totals cast to `f64`.
#[derive(Clone, Debug)]
pub struct MatchResult {
    pub a_idx: usize,
    pub b_idx: usize,
    pub rounds: u32,
    pub a_total: i64,
    pub b_total: i64,
    pub a_adjusted_total: f64,
    pub b_adjusted_total: f64,
    pub repetition: u32,
    pub match_id: usize,
}

/// Which side of a match a strategy occupies (first-mover vs second-mover).
#[derive(Copy, Clone, Debug)]
pub enum MatchRole {
    A,
    B,
}

impl MatchRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
}

/// A scheduled matchup: two strategy indices, a repetition, and a global match id.
#[derive(Clone, Debug)]
pub struct Matchup {
    pub match_id: usize,
    pub a_idx: usize,
    pub b_idx: usize,
    pub repetition: u32,
}

/// Mutable state for a single match in progress.
///
/// Holds the two strategy instances, shared history buffer, noise RNG,
/// per-round trace buffers, and cumulative scores. Created at the start
/// of each match and consumed when the match finishes.
pub struct MatchSession {
    pub matchup: Matchup,
    pub history: History,
    pub a_strategy: Box<dyn Strategy>,
    pub b_strategy: Box<dyn Strategy>,
    pub noise_rng: SplitMix64,
    pub history_actions_a: String,
    pub history_actions_b: String,
    pub history_halted_a: String,
    pub history_halted_b: String,
    pub history_scores: String,
    pub history_payoffs: Vec<[i32; 2]>,
    pub round: u32,
    pub rounds_total: u32,
    pub a_total: i64,
    pub b_total: i64,
    pub a_crashed: bool,
    pub b_crashed: bool,
    pub record_history: bool,
    pub record_trace: bool,
}

/// Actions and payoffs from a single round of play.
#[derive(Clone, Debug)]
pub struct RoundSnapshot {
    pub a_action: Action,
    pub b_action: Action,
    pub a_halted: bool,
    pub b_halted: bool,
    pub a_payoff: i32,
    pub b_payoff: i32,
}

/// Round snapshot plus crash flags, returned from `play_round_core`.
pub struct RoundOutcome {
    pub snapshot: RoundSnapshot,
    pub a_crash_now: bool,
    pub b_crash_now: bool,
}

/// Complete outcome of a match: result, crash flags, TM stats, and last round.
pub struct MatchOutcome {
    pub result: MatchResult,
    pub a_crashed: bool,
    pub b_crashed: bool,
    pub a_tm_stats: Option<TmRunStats>,
    pub b_tm_stats: Option<TmRunStats>,
    pub last_round: Option<RoundSnapshot>,
}
