use super::match_state::RoundSnapshot;
use crate::game::{Action, Outcome};
use crate::output::RuntimeAcceleratorStats;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct TournamentProgress {
    pub match_index: usize,
    pub total_matches: usize,
    pub round: u32,
    pub rounds: u32,
    pub match_complete: bool,
    pub a: String,
    pub b: String,
    pub total_payoff_a: i64,
    pub total_payoff_b: i64,
    pub last_action_a: Option<Action>,
    pub last_action_b: Option<Action>,
    pub last_payoff_a: Option<i32>,
    pub last_payoff_b: Option<i32>,
    pub last_halted_a: Option<bool>,
    pub last_halted_b: Option<bool>,
    pub last_outcome: Option<Outcome>,
    pub runtime: RuntimeAcceleratorStats,
}

impl TournamentProgress {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        match_index: usize,
        total_matches: usize,
        current_round: u32,
        total_rounds: u32,
        match_complete: bool,
        strategy_a: String,
        strategy_b: String,
        cumulative_payoff_a: i64,
        cumulative_payoff_b: i64,
        last_round: Option<&RoundSnapshot>,
        runtime: RuntimeAcceleratorStats,
    ) -> Self {
        Self {
            match_index,
            total_matches,
            round: current_round,
            rounds: total_rounds,
            match_complete,
            a: strategy_a,
            b: strategy_b,
            total_payoff_a: cumulative_payoff_a,
            total_payoff_b: cumulative_payoff_b,
            last_action_a: last_round.map(|r| r.a_action),
            last_action_b: last_round.map(|r| r.b_action),
            last_payoff_a: last_round.map(|r| r.a_payoff),
            last_payoff_b: last_round.map(|r| r.b_payoff),
            last_halted_a: last_round.map(|r| r.a_halted),
            last_halted_b: last_round.map(|r| r.b_halted),
            last_outcome: last_round.map(|r| Outcome::from_actions(r.a_action, r.b_action)),
            runtime,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MatchSnapshot {
    pub match_index: usize,
    pub total_matches: usize,
    pub round: u32,
    pub rounds: u32,
    pub a: String,
    pub b: String,
    pub a_score: i64,
    pub b_score: i64,
    // Encoded outcome history: one digit char (`'0'`..`'3'`) per round.
    pub outcomes: String,
    pub payoffs: Vec<[i32; 2]>,
    // Per-round halting flags for each strategy (`'0'` or `'1'` per round).
    pub a_halted: String,
    pub b_halted: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchHistoryPreview {
    pub match_index: usize,
    pub total_matches: usize,
    pub a: String,
    pub b: String,
    pub rounds_total: u32,
    #[serde(alias = "outcomes_prefix")]
    pub outcomes: String,
}

impl MatchHistoryPreview {
    // Maximum rounds shown in the TUI preview widget; loaders trim to this cap
    // before the popup renders so widget sizing stays stable for huge runs.
    pub const DISPLAY_ROUND_CAP: usize = 500;

    pub fn preview_rounds(&self) -> usize {
        self.outcomes.len().min(Self::DISPLAY_ROUND_CAP)
    }

    pub fn preview_outcomes(&self) -> &str {
        let end = self.preview_rounds();
        self.outcomes.get(..end).unwrap_or(self.outcomes.as_str())
    }
}
