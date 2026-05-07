use std::collections::HashMap;

use nit_gol::{EdgeMode, Grid, Rule};

use crate::seed::SeedEncoderId;

use super::{EncoderScore, GrowthClass};

pub(super) const MAX_GENERATIONS: u32 = 3000;

/// Maximum lift (in generations) the soft bottleneck rule can apply.
/// Caps at 200 so Replicator (2001+) still requires genuine quality across all
/// encoders — the lift can bump you up roughly one tier at most.
pub(super) const SOFT_BOTTLENECK_MAX_LIFT: u32 = 200;

const GRID_MIN: usize = 32;
const GRID_MID: usize = 48;
const GRID_MAX: usize = 64;

const SMALL_FILE_BYTES: usize = 2048;
const MEDIUM_FILE_BYTES: usize = 10240;

const EARLY_POP_FRACTION: u32 = 10;
const LATE_POP_NUM: u32 = 4;
const LATE_POP_DEN: u32 = 5;

const EXPANDING_RATIO: f32 = 1.15;
const COLLAPSING_RATIO: f32 = 0.85;

/// AST-driven encoder IDs used for tier determination.
pub(super) const AST_ENCODERS: [SeedEncoderId; 3] = [
    SeedEncoderId::TokenSpectrum,
    SeedEncoderId::AstStructure,
    SeedEncoderId::ComplexityField,
];

/// Three AST-driven (determine tier) + one hybrid (structural). Byte-level
/// encoders (ascii_bytes, hilbert_bits, lifehash16) are excluded — they
/// measure surface byte patterns that add noise to quality signals and
/// cannot be meaningfully improved through code changes.
pub(super) const QUALITY_ENCODERS: [SeedEncoderId; 4] = [
    SeedEncoderId::TokenSpectrum,
    SeedEncoderId::AstStructure,
    SeedEncoderId::ComplexityField,
    SeedEncoderId::Structural,
];

pub(super) fn adaptive_grid_size(file_bytes: usize) -> usize {
    if file_bytes <= SMALL_FILE_BYTES {
        GRID_MIN
    } else if file_bytes <= MEDIUM_FILE_BYTES {
        GRID_MID
    } else {
        GRID_MAX
    }
}

pub(super) fn simulate_gol(
    encoder: SeedEncoderId,
    initial: &Grid,
    density: f32,
    components: usize,
    rule: Rule,
    max_generations: u32,
) -> EncoderScore {
    let mut grid = initial.clone();
    let mut seen: HashMap<u64, u32> = HashMap::new();
    let mut peak_population: u32 = grid.alive_count() as u32;
    let hash0 = grid.hash();
    seen.insert(hash0, 0);

    let mut generations_survived: u32 = 0;
    let mut cycle_period: Option<u32> = None;
    let mut early_pop: u32 = 0;
    let mut late_pop: u32 = 0;
    let mut died = false;

    for gen in 1..=max_generations {
        grid = nit_gol::step::step(&grid, rule, EdgeMode::Dead);
        let alive = grid.alive_count() as u32;
        if alive > peak_population {
            peak_population = alive;
        }
        if gen == max_generations / EARLY_POP_FRACTION {
            early_pop = alive;
        }
        if gen == max_generations * LATE_POP_NUM / LATE_POP_DEN {
            late_pop = alive;
        }
        if alive == 0 {
            generations_survived = gen;
            died = true;
            break;
        }
        let h = grid.hash();
        if let Some(&first_seen) = seen.get(&h) {
            generations_survived = gen;
            cycle_period = Some(gen - first_seen);
            late_pop = alive;
            break;
        }
        seen.insert(h, gen);
    }
    if cycle_period.is_none() && generations_survived == 0 {
        generations_survived = max_generations;
    }

    EncoderScore {
        encoder,
        density,
        components,
        generations_survived,
        peak_population,
        cycle_period,
        growth_class: classify_growth(early_pop, late_pop, died),
    }
}

fn classify_growth(early_pop: u32, late_pop: u32, died: bool) -> GrowthClass {
    if died {
        return GrowthClass::Extinct;
    }
    if early_pop == 0 || late_pop == 0 {
        return GrowthClass::Stable;
    }
    let ratio = late_pop as f32 / early_pop as f32;
    if ratio > EXPANDING_RATIO {
        GrowthClass::Expanding
    } else if ratio < COLLAPSING_RATIO {
        GrowthClass::Collapsing
    } else {
        GrowthClass::Stable
    }
}

/// Cross-encoder consistency from actionable encoders only (AST + structural).
/// Byte-level encoders are excluded — they add noise that doesn't reflect
/// code quality the agent can act on.
pub(super) fn compute_consistency(scores: &[EncoderScore]) -> f32 {
    let gens: Vec<f64> = scores
        .iter()
        .filter(|s| QUALITY_ENCODERS.contains(&s.encoder))
        .map(|s| s.generations_survived as f64)
        .collect();
    if gens.is_empty() {
        return 0.0;
    }
    let mean = gens.iter().sum::<f64>() / gens.len() as f64;
    if mean == 0.0 {
        return 0.0;
    }
    let variance = gens.iter().map(|g| (g - mean).powi(2)).sum::<f64>() / gens.len() as f64;
    let std_dev = variance.sqrt();
    (1.0 - (std_dev / mean) as f32).clamp(0.0, 1.0)
}
