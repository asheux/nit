//! Rule evaluation and scoring for comparative analysis.
//!
//! Runs a simulation under a given rule and scores it based on
//! longevity, oscillation period, population dynamics, and density.

use std::collections::HashMap;

use crate::{step::step, EdgeMode, Grid, Rule};

/// Complete evaluation result for a single rule on a given seed.
///
/// Carries both the scalar score and the supporting metrics
/// (period, transient length, population statistics) that produced it,
/// plus the final grid state for downstream inspection.
#[derive(Clone, Debug)]
#[must_use]
pub struct RuleEvaluation {
    pub rule: Rule,
    /// Composite quality score — higher is better.
    pub score: f32,
    /// Detected oscillation period, if any.
    pub period: Option<u32>,
    /// Generations elapsed before the first repeat or extinction.
    pub transient: u32,
    pub avg_population: f32,
    pub max_population: u32,
    pub alive_end: u32,
    /// Always equal to `transient`; retained for API stability.
    pub steps: u32,
    pub final_grid: Grid,
}

/// Lightweight score summary used for ranking when the final grid is not needed.
#[derive(Clone, Debug)]
#[must_use]
pub struct RuleScore {
    pub rule: Rule,
    pub score: f32,
    pub period: Option<u32>,
    pub transient: u32,
    pub avg_population: f32,
    pub max_population: u32,
    pub alive_end: u32,
}

/// Raw simulation output, pre-scoring.
struct SimMetrics {
    final_grid: Grid,
    period: Option<u32>,
    transient: u32,
    avg_population: f32,
    max_population: u32,
    alive_end: u32,
}

/// Simulate a rule on `seed` for up to `max_generations` and score it.
///
/// The simulation halts early on extinction (zero alive cells) or when
/// a previously seen grid hash is encountered (cycle detection). The
/// returned [`RuleEvaluation`] carries both the score and supporting
/// metrics.
pub fn evaluate_rule(
    seed: &Grid,
    rule: Rule,
    edge: EdgeMode,
    max_generations: u32,
) -> RuleEvaluation {
    let metrics = simulate_with_cycle_detection(seed, rule, edge, max_generations);
    let score = score_rule(
        seed.width() * seed.height(),
        metrics.transient,
        metrics.period,
        metrics.avg_population,
        metrics.max_population,
        metrics.alive_end,
    );
    RuleEvaluation {
        rule,
        score,
        period: metrics.period,
        transient: metrics.transient,
        avg_population: metrics.avg_population,
        max_population: metrics.max_population,
        alive_end: metrics.alive_end,
        steps: metrics.transient,
        final_grid: metrics.final_grid,
    }
}

/// Step the seed forward, tracking population stats and stopping on a
/// repeat hash (cycle) or extinction.
///
/// On cycle detection the final grid is the matched state (not stepped
/// forward) so downstream callers can inspect the oscillator itself.
fn simulate_with_cycle_detection(
    seed: &Grid,
    rule: Rule,
    edge: EdgeMode,
    max_generations: u32,
) -> SimMetrics {
    let mut grid = seed.clone();
    let mut seen: HashMap<u64, u32> = HashMap::new();
    let mut sum_population: u64 = 0;
    let mut max_population: u32 = 0;
    let mut period = None;
    let mut transient = 0;
    let mut alive_end = 0;

    for generation in 0..max_generations {
        let hash = grid.hash();
        if let Some(prev) = seen.get(&hash) {
            period = Some(generation.saturating_sub(*prev));
            transient = generation;
            alive_end = grid.alive_count() as u32;
            break;
        }
        seen.insert(hash, generation);
        let alive = grid.alive_count() as u32;
        sum_population += alive as u64;
        max_population = max_population.max(alive);
        alive_end = alive;
        if alive == 0 {
            transient = generation + 1;
            break;
        }
        grid = step(&grid, rule, edge);
        transient = generation + 1;
    }

    let avg_population = if transient > 0 {
        sum_population as f32 / transient as f32
    } else {
        0.0
    };

    SimMetrics {
        final_grid: grid,
        period,
        transient,
        avg_population,
        max_population,
        alive_end,
    }
}

/// Compute a composite score from simulation metrics.
///
/// Rewards longevity and periodic behavior while penalizing
/// extinction and near-total saturation of the grid.
#[must_use]
pub(crate) fn score_rule(
    grid_area: usize,
    transient: u32,
    period: Option<u32>,
    avg_population: f32,
    max_population: u32,
    alive_end: u32,
) -> f32 {
    let mut score = transient as f32 * 2.0;
    if let Some(p) = period {
        score += 8.0 * (1.0 + p as f32).log2();
    }
    score += 0.5 * avg_population;
    if alive_end == 0 {
        score -= 12.0;
    }
    if grid_area > 0 && max_population as usize > (grid_area as f32 * 0.92) as usize {
        score -= 10.0;
    }
    score
}
