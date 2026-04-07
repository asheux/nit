//! Rule evaluation and scoring for comparative analysis.
//!
//! Runs a simulation under a given rule and scores it based on
//! longevity, oscillation period, population dynamics, and density.

use std::collections::HashMap;

use crate::{step::step, EdgeMode, Grid, Rule};

/// Complete evaluation result for a single rule on a given seed.
///
/// Contains both the scalar score and the supporting metrics
/// (period, transient length, population statistics) that produced it,
/// along with the final grid state for downstream inspection.
#[derive(Clone, Debug)]
pub struct RuleEvaluation {
    /// The rule that was evaluated.
    pub rule: Rule,
    /// Composite quality score (higher is better).
    pub score: f32,
    /// Detected oscillation period, if any.
    pub period: Option<u32>,
    /// Number of generations before the first repeat or extinction.
    pub transient: u32,
    /// Mean alive-cell count across all simulated generations.
    pub avg_population: f32,
    /// Peak alive-cell count observed during the run.
    pub max_population: u32,
    /// Alive-cell count at the final generation.
    pub alive_end: u32,
    /// Total generations simulated before halting.
    pub steps: u32,
    /// Grid state at the end of the simulation.
    pub final_grid: Grid,
}

/// Lightweight score summary without the final grid.
///
/// Used when only the ranking metrics are needed and carrying
/// the full grid would be wasteful.
#[derive(Clone, Debug)]
pub struct RuleScore {
    pub rule: Rule,
    pub score: f32,
    pub period: Option<u32>,
    pub transient: u32,
    pub avg_population: f32,
    pub max_population: u32,
    pub alive_end: u32,
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
    let mut grid = seed.clone();
    let mut seen: HashMap<u64, u32> = HashMap::new();
    let mut sum_population: u64 = 0;
    let mut max_population: u32 = 0;
    let mut period = None;
    let mut transient = 0;
    let mut alive_end = 0;

    for gen in 0..max_generations {
        let hash = grid.hash();
        if let Some(prev) = seen.get(&hash) {
            period = Some(gen.saturating_sub(*prev));
            transient = gen;
            alive_end = grid.alive_count() as u32;
            break;
        }
        seen.insert(hash, gen);
        let alive = grid.alive_count() as u32;
        sum_population += alive as u64;
        if alive > max_population {
            max_population = alive;
        }
        alive_end = alive;
        if alive == 0 {
            transient = gen + 1;
            break;
        }
        grid = step(&grid, rule, edge);
        transient = gen + 1;
    }

    let avg_population = if transient > 0 {
        sum_population as f32 / transient as f32
    } else {
        0.0
    };
    let score = score_rule(
        seed.width() * seed.height(),
        transient,
        period,
        avg_population,
        max_population,
        alive_end,
    );

    RuleEvaluation {
        rule,
        score,
        period,
        transient,
        avg_population,
        max_population,
        alive_end,
        steps: transient,
        final_grid: grid,
    }
}

/// Compute a composite score from simulation metrics.
///
/// Rewards longevity and periodic behavior while penalizing
/// extinction and near-total saturation of the grid.
pub fn score_rule(
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
