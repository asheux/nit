//! Tournament result accumulation and final ranking computation.

use std::cmp::Ordering;

use super::halting::compare_scores;
use super::session::tm_metrics_from_stats;
use super::types::{MatchResult, PairStats, StrategyStats, TournamentAccumulator};
use crate::config::{ScoreAggregation, StrategySpec};
use crate::output::{DominanceEdge, PairwiseResult, StrategyResult, TournamentResults};
use crate::strategy::TmRunStats;

impl TournamentAccumulator {
    pub(super) fn new(
        n: usize,
        use_adjusted: bool,
        score_aggregation: ScoreAggregation,
        store_pairwise: bool,
    ) -> Self {
        Self {
            strategies: vec![
                StrategyStats {
                    total: 0,
                    adjusted_total: 0.0,
                    score_samples: 0,
                    matches: 0,
                    wins: 0,
                    losses: 0,
                    draws: 0,
                    crash_count: 0,
                    crashed: false,
                    tm_stats: None,
                };
                n
            ],
            pairwise: store_pairwise.then(|| vec![vec![PairStats::default(); n]; n]),
            use_adjusted,
            score_aggregation,
        }
    }

    /// Fold a single match result into running totals. Self-play (same index on
    /// both sides) credits both roles to the same strategy entry.
    pub(super) fn apply_match(
        &mut self,
        result: MatchResult,
        a_crashed: bool,
        b_crashed: bool,
        a_tm_stats: Option<TmRunStats>,
        b_tm_stats: Option<TmRunStats>,
    ) {
        let (a_outcome, b_outcome) = if self.use_adjusted {
            (result.a_adjusted_total, result.b_adjusted_total)
        } else {
            (result.a_total as f64, result.b_total as f64)
        };
        let outcome_order = compare_scores(a_outcome, b_outcome);
        let score_samples = u64::from(result.rounds);
        if result.a_idx == result.b_idx {
            let stats = &mut self.strategies[result.a_idx];
            stats.total += result.a_total + result.b_total;
            stats.adjusted_total += result.a_adjusted_total + result.b_adjusted_total;
            stats.score_samples += score_samples.saturating_mul(2);
            stats.matches += 2;
            match outcome_order {
                Ordering::Greater | Ordering::Less => {
                    stats.wins += 1;
                    stats.losses += 1;
                }
                Ordering::Equal => {
                    stats.draws += 2;
                }
            }
            if a_crashed || b_crashed {
                stats.crashed = true;
            }
            if let Some(pairwise) = self.pairwise.as_mut() {
                let pair = &mut pairwise[result.a_idx][result.b_idx];
                pair.a_total += result.a_total;
                pair.b_total += result.b_total;
                pair.a_adjusted_total += result.a_adjusted_total;
                pair.b_adjusted_total += result.b_adjusted_total;
                match outcome_order {
                    Ordering::Greater => pair.a_wins += 1,
                    Ordering::Less => pair.b_wins += 1,
                    Ordering::Equal => pair.draws += 1,
                }
            }
            if let Some(tm_stats) = a_tm_stats.as_ref() {
                let entry = stats.tm_stats.get_or_insert_with(TmRunStats::default);
                entry.merge(tm_stats);
            }
            if let Some(tm_stats) = b_tm_stats.as_ref() {
                let entry = stats.tm_stats.get_or_insert_with(TmRunStats::default);
                entry.merge(tm_stats);
            }
            return;
        }
        let (a_stats, b_stats) = if result.a_idx < result.b_idx {
            let (left, right) = self.strategies.split_at_mut(result.b_idx);
            let a_stats = &mut left[result.a_idx];
            let b_stats = &mut right[0];
            (a_stats, b_stats)
        } else {
            let (left, right) = self.strategies.split_at_mut(result.a_idx);
            let b_stats = &mut left[result.b_idx];
            let a_stats = &mut right[0];
            (a_stats, b_stats)
        };
        a_stats.total += result.a_total;
        b_stats.total += result.b_total;
        a_stats.adjusted_total += result.a_adjusted_total;
        b_stats.adjusted_total += result.b_adjusted_total;
        a_stats.score_samples += score_samples;
        b_stats.score_samples += score_samples;
        a_stats.matches += 1;
        b_stats.matches += 1;
        if a_crashed {
            a_stats.crashed = true;
        }
        if b_crashed {
            b_stats.crashed = true;
        }
        if let Some(tm_stats) = a_tm_stats.as_ref() {
            let entry = a_stats.tm_stats.get_or_insert_with(TmRunStats::default);
            entry.merge(tm_stats);
        }
        if let Some(tm_stats) = b_tm_stats.as_ref() {
            let entry = b_stats.tm_stats.get_or_insert_with(TmRunStats::default);
            entry.merge(tm_stats);
        }

        match outcome_order {
            Ordering::Greater => {
                a_stats.wins += 1;
                b_stats.losses += 1;
            }
            Ordering::Less => {
                b_stats.wins += 1;
                a_stats.losses += 1;
            }
            Ordering::Equal => {
                a_stats.draws += 1;
                b_stats.draws += 1;
            }
        }

        if let Some(pairwise) = self.pairwise.as_mut() {
            let pair = &mut pairwise[result.a_idx][result.b_idx];
            pair.a_total += result.a_total;
            pair.b_total += result.b_total;
            pair.a_adjusted_total += result.a_adjusted_total;
            pair.b_adjusted_total += result.b_adjusted_total;
            match outcome_order {
                Ordering::Greater => pair.a_wins += 1,
                Ordering::Less => pair.b_wins += 1,
                Ordering::Equal => pair.draws += 1,
            }

            if result.a_idx != result.b_idx {
                let reverse = &mut pairwise[result.b_idx][result.a_idx];
                reverse.a_total += result.b_total;
                reverse.b_total += result.a_total;
                reverse.a_adjusted_total += result.b_adjusted_total;
                reverse.b_adjusted_total += result.a_adjusted_total;
                match compare_scores(b_outcome, a_outcome) {
                    Ordering::Greater => reverse.a_wins += 1,
                    Ordering::Less => reverse.b_wins += 1,
                    Ordering::Equal => reverse.draws += 1,
                }
            }
        }
    }

    fn build_ranking(&self, specs: &[StrategySpec]) -> Vec<StrategyResult> {
        let mut ranking: Vec<StrategyResult> = self
            .strategies
            .iter()
            .enumerate()
            .map(|(idx, stats)| {
                let score_samples = stats.score_samples.max(1);
                let adjusted_avg = stats.adjusted_total / score_samples as f64;
                StrategyResult {
                    id: specs[idx].id.clone(),
                    name: specs[idx].name.clone(),
                    total_payoff: stats.total,
                    average_payoff: stats.total as f64 / score_samples as f64,
                    adjusted_total_payoff: Some(stats.adjusted_total),
                    adjusted_average_payoff: Some(adjusted_avg),
                    matches: stats.matches,
                    wins: stats.wins,
                    losses: stats.losses,
                    draws: stats.draws,
                    crashed: stats.crashed,
                    crash_count: stats.crash_count,
                    tm_metrics: stats.tm_stats.as_ref().map(tm_metrics_from_stats),
                }
            })
            .collect();
        let prefer_adjusted = self.use_adjusted;
        let aggregation = self.score_aggregation;
        ranking.sort_by(|a, b| {
            let a_score = a.score(aggregation, prefer_adjusted);
            let b_score = b.score(aggregation, prefer_adjusted);
            b_score.partial_cmp(&a_score).unwrap_or(Ordering::Equal)
        });
        ranking
    }

    /// Produce a lightweight leaderboard (ranking only, no pairwise or dominance data).
    ///
    /// Used by the TUI for the live leaderboard display during tournament execution.
    pub(super) fn leaderboard(&self, specs: &[StrategySpec]) -> TournamentResults {
        TournamentResults {
            ranking: self.build_ranking(specs),
            pairwise: Vec::new(),
            dominance: Vec::new(),
        }
    }

    /// Produce the final [`TournamentResults`] with ranking, pairwise table, and
    /// dominance edges.
    ///
    /// Called once after all matches have been accumulated.
    pub(super) fn finalize(&self, specs: &[StrategySpec]) -> TournamentResults {
        let ranking = self.build_ranking(specs);

        let mut pairwise = Vec::new();
        if let Some(rows) = self.pairwise.as_ref() {
            for (i, row) in rows.iter().enumerate() {
                for (j, pair) in row.iter().enumerate() {
                    if i >= j {
                        continue;
                    }
                    if pair.a_total == 0
                        && pair.b_total == 0
                        && pair.a_wins == 0
                        && pair.b_wins == 0
                        && pair.draws == 0
                    {
                        continue;
                    }
                    pairwise.push(PairwiseResult {
                        a: specs[i].id.clone(),
                        b: specs[j].id.clone(),
                        a_total: pair.a_total,
                        b_total: pair.b_total,
                        a_adjusted_total: Some(pair.a_adjusted_total),
                        b_adjusted_total: Some(pair.b_adjusted_total),
                        a_wins: pair.a_wins,
                        b_wins: pair.b_wins,
                        draws: pair.draws,
                    });
                }
            }
        }

        let mut dominance = Vec::new();
        for pair in &pairwise {
            if pair.a_total > pair.b_total {
                dominance.push(DominanceEdge {
                    winner: pair.a.clone(),
                    loser: pair.b.clone(),
                });
            } else if pair.b_total > pair.a_total {
                dominance.push(DominanceEdge {
                    winner: pair.b.clone(),
                    loser: pair.a.clone(),
                });
            }
        }

        TournamentResults {
            ranking,
            pairwise,
            dominance,
        }
    }
}
