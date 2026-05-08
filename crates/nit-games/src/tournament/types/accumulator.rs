use crate::config::ScoreAggregation;
use crate::strategy::TmRunStats;

#[derive(Clone, Debug)]
pub struct StrategyStats {
    pub total: i64,
    pub adjusted_total: f64,
    pub score_samples: u64,
    pub matches: u32,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub crash_count: u32,
    pub crashed: bool,
    pub tm_stats: Option<TmRunStats>,
}

#[derive(Clone, Debug, Default)]
pub struct PairStats {
    pub a_total: i64,
    pub b_total: i64,
    pub a_adjusted_total: f64,
    pub b_adjusted_total: f64,
    pub a_wins: u32,
    pub b_wins: u32,
    pub draws: u32,
}

impl PairStats {
    pub fn is_empty(&self) -> bool {
        self.a_total == 0
            && self.b_total == 0
            && self.a_wins == 0
            && self.b_wins == 0
            && self.draws == 0
    }
}

pub struct TournamentAccumulator {
    pub strategies: Vec<StrategyStats>,
    pub pairwise: Option<Vec<Vec<PairStats>>>,
    pub use_adjusted: bool,
    pub score_aggregation: ScoreAggregation,
}
