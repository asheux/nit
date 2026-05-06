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
const GRID_MAX: usize = 64;

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

/// Scales from 32x32 for small files up to 64x64 for large files,
/// preserving structural fidelity without blowing up simulation cost.
pub(super) fn adaptive_grid_size(file_bytes: usize) -> usize {
    match file_bytes {
        0..=2048 => GRID_MIN, // <= 2KB: 32x32 (1,024 cells)
        2049..=10240 => 48,   // 2-10KB: 48x48 (2,304 cells)
        _ => GRID_MAX,        // > 10KB: 64x64 (4,096 cells)
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
        if gen == max_generations / 10 {
            early_pop = alive;
        }
        if gen == max_generations * 4 / 5 {
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

    let growth_class = if died {
        GrowthClass::Extinct
    } else if early_pop == 0 || late_pop == 0 {
        GrowthClass::Stable
    } else {
        let ratio = late_pop as f32 / early_pop as f32;
        if ratio > 1.15 {
            GrowthClass::Expanding
        } else if ratio < 0.85 {
            GrowthClass::Collapsing
        } else {
            GrowthClass::Stable
        }
    };

    EncoderScore {
        encoder,
        density,
        components,
        generations_survived,
        peak_population,
        cycle_period,
        growth_class,
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
