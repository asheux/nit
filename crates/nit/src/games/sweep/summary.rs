use std::collections::HashMap;

use serde::Serialize;

use nit_games::ScoreAggregation;

pub(super) const SWEEP_SCHEMA_VERSION: u32 = 1;

#[derive(Serialize)]
pub(super) struct SweepSummary {
    pub schema_version: u32,
    pub timestamp: String,
    pub seed: u64,
    pub config_path: String,
    pub grid: SweepGrid,
    pub cells: Vec<SweepCellSummary>,
    pub aggregate: SweepAggregate,
}

#[derive(Serialize)]
pub(super) struct SweepGrid {
    pub rounds: Vec<u32>,
    pub noise: Vec<f32>,
    pub repetitions: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payoff_preset: Option<String>,
    pub payoff_r: Vec<i32>,
    pub payoff_s: Vec<i32>,
    pub payoff_t: Vec<i32>,
    pub payoff_p: Vec<i32>,
}

#[derive(Serialize)]
pub(super) struct SweepCellSummary {
    pub cell_id: usize,
    pub rounds: u32,
    pub noise: f32,
    pub repetitions: u32,
    pub payoff_r: i32,
    pub payoff_s: i32,
    pub payoff_t: i32,
    pub payoff_p: i32,
    pub seed: u64,
    pub run_id: String,
    pub run_dir: String,
    pub summary_path: String,
    pub top_strategy: String,
    pub top_strategies: Vec<SweepTopEntry>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub skipped: bool,
}

#[derive(Serialize)]
pub(super) struct SweepAggregate {
    pub score_aggregation: ScoreAggregation,
    pub adjusted_scores: bool,
    pub strategies: Vec<SweepStrategyAggregate>,
}

#[derive(Serialize)]
pub(super) struct SweepStrategyAggregate {
    pub id: String,
    pub mean_score: f64,
    pub std_score: f64,
    pub top1_count: u32,
}

#[derive(Serialize)]
pub(super) struct SweepTopEntry {
    pub id: String,
    pub score: f64,
}

pub(super) fn compute_sweep_aggregates(
    scores_by_strategy: HashMap<String, Vec<f64>>,
    top_counts: HashMap<String, u32>,
) -> Vec<SweepStrategyAggregate> {
    let mut rankings = Vec::new();
    for (id, scores) in scores_by_strategy {
        let n = scores.len() as f64;
        let mean = scores.iter().sum::<f64>() / n.max(1.0);
        let variance = scores.iter().map(|s| (*s - mean).powi(2)).sum::<f64>() / n.max(1.0);
        let top1 = top_counts.get(&id).copied().unwrap_or(0);
        rankings.push(SweepStrategyAggregate {
            id,
            mean_score: mean,
            std_score: variance.sqrt(),
            top1_count: top1,
        });
    }
    rankings.sort_by(|a, b| b.mean_score.total_cmp(&a.mean_score));
    rankings
}
