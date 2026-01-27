use crate::config::{NormalizedConfig, StrategySpecKind};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategyDefinition {
    pub id: String,
    pub name: Option<String>,
    #[serde(flatten)]
    pub kind: StrategySpecKind,
    pub rng_seed_a: Option<u64>,
    pub rng_seed_b: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategyResult {
    pub id: String,
    pub name: Option<String>,
    pub total_payoff: i64,
    pub average_payoff: f64,
    pub matches: u32,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub crashed: bool,
    pub crash_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairwiseResult {
    pub a: String,
    pub b: String,
    pub a_total: i64,
    pub b_total: i64,
    pub a_wins: u32,
    pub b_wins: u32,
    pub draws: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DominanceEdge {
    pub winner: String,
    pub loser: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TournamentResults {
    pub ranking: Vec<StrategyResult>,
    pub pairwise: Vec<PairwiseResult>,
    pub dominance: Vec<DominanceEdge>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunSummary {
    pub schema_version: u32,
    pub timestamp: String,
    pub seed: u64,
    pub config: NormalizedConfig,
    pub strategies: Vec<StrategyDefinition>,
    pub results: TournamentResults,
    pub event_log: Option<String>,
    pub history_log: Option<String>,
}
