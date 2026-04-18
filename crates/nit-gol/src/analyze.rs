//! Rule evaluation and scoring for comparative analysis.
//!
//! Runs a simulation under a given rule and scores it based on
//! longevity, oscillation period, population dynamics, and density.

use std::collections::HashMap;

use crate::{step::step, EdgeMode, Grid, Rule};

// Score weights used by `score_rule`; tuned empirically and frozen so
// ranked-rule logs remain comparable across runs.
const SCORE_TRANSIENT_WEIGHT: f32 = 2.0;
const SCORE_PERIOD_LOG_WEIGHT: f32 = 8.0;
const SCORE_AVG_POP_WEIGHT: f32 = 0.5;
const SCORE_EXTINCTION_PENALTY: f32 = 12.0;
const SCORE_SATURATION_PENALTY: f32 = 10.0;

/// Alive-cell fraction above which a rule is treated as saturating and
/// the score is docked — prevents runaway "all-on" rules from winning.
const SATURATION_THRESHOLD: f32 = 0.92;

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

/// Running totals for per-generation population statistics.
struct PopulationTracker {
    running_sum: u64,
    peak_alive: u32,
    most_recent: u32,
}

impl PopulationTracker {
    fn new() -> Self {
        Self {
            running_sum: 0,
            peak_alive: 0,
            most_recent: 0,
        }
    }

    fn record(&mut self, alive: u32) {
        self.running_sum += u64::from(alive);
        if alive > self.peak_alive {
            self.peak_alive = alive;
        }
        self.most_recent = alive;
    }

    fn mean_over(&self, ticks: u32) -> f32 {
        if ticks == 0 {
            return 0.0;
        }
        self.running_sum as f32 / ticks as f32
    }
}

/// Simulate a rule on `seed` for up to `max_generations` and score it.
///
/// The simulation halts early on extinction (zero alive cells) or when
/// a previously seen grid hash is encountered (cycle detection).
pub fn evaluate_rule(
    seed: &Grid,
    rule: Rule,
    edge: EdgeMode,
    max_generations: u32,
) -> RuleEvaluation {
    let outcome = simulate(seed, rule, edge, max_generations);
    let area = seed.width() * seed.height();
    let avg = outcome.population.mean_over(outcome.transient);
    let score = score_rule(
        area,
        outcome.transient,
        outcome.period,
        avg,
        outcome.population.peak_alive,
        outcome.population.most_recent,
    );
    RuleEvaluation {
        rule,
        score,
        period: outcome.period,
        transient: outcome.transient,
        avg_population: avg,
        max_population: outcome.population.peak_alive,
        alive_end: outcome.population.most_recent,
        steps: outcome.transient,
        final_grid: outcome.final_grid,
    }
}

struct SimOutcome {
    final_grid: Grid,
    period: Option<u32>,
    transient: u32,
    population: PopulationTracker,
}

/// Step the seed forward, tracking population stats and stopping on a
/// repeat hash (cycle) or extinction.
///
/// On cycle detection the final grid is the matched state (not stepped
/// forward) so downstream callers can inspect the oscillator itself.
fn simulate(seed: &Grid, rule: Rule, edge: EdgeMode, max_generations: u32) -> SimOutcome {
    let mut grid = seed.clone();
    let mut seen: HashMap<u64, u32> = HashMap::new();
    let mut population = PopulationTracker::new();
    let mut transient = 0u32;

    for generation in 0..max_generations {
        let hash = grid.hash();
        if let Some(&prev) = seen.get(&hash) {
            let alive_now = u32_alive(&grid);
            population.most_recent = alive_now;
            return SimOutcome {
                final_grid: grid,
                period: Some(generation.saturating_sub(prev)),
                transient: generation,
                population,
            };
        }
        seen.insert(hash, generation);

        let alive = u32_alive(&grid);
        population.record(alive);
        transient = generation + 1;
        if alive == 0 {
            break;
        }
        grid = step(&grid, rule, edge);
    }

    SimOutcome {
        final_grid: grid,
        period: None,
        transient,
        population,
    }
}

/// Composite score: rewards longevity and periodic behavior, penalizes
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
    let mut score = transient as f32 * SCORE_TRANSIENT_WEIGHT;
    if let Some(p) = period {
        score += SCORE_PERIOD_LOG_WEIGHT * (1.0 + p as f32).log2();
    }
    score += SCORE_AVG_POP_WEIGHT * avg_population;
    if alive_end == 0 {
        score -= SCORE_EXTINCTION_PENALTY;
    }
    if saturated(grid_area, max_population) {
        score -= SCORE_SATURATION_PENALTY;
    }
    score
}

fn saturated(grid_area: usize, max_population: u32) -> bool {
    if grid_area == 0 {
        return false;
    }
    let cap = (grid_area as f32 * SATURATION_THRESHOLD) as usize;
    max_population as usize > cap
}

/// `Grid::alive_count` is `usize`; population stats use `u32` throughout
/// the public API, so saturate rather than widening — any grid large
/// enough to exceed `u32::MAX` cells is well beyond what we simulate.
fn u32_alive(grid: &Grid) -> u32 {
    u32::try_from(grid.alive_count()).unwrap_or(u32::MAX)
}
