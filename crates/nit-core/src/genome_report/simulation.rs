use std::collections::HashMap;

use nit_gol::{EdgeMode, Grid, Rule};

use crate::seed::SeedEncoderId;

use super::{EncoderScore, GrowthClass};

pub(super) const MAX_GENERATIONS: u32 = 3000;

/// Maximum lift (in generations) the soft bottleneck rule can apply.
/// Caps at 200 so Replicator (2001+) still requires genuine quality across all
/// encoders — the lift can bump you up roughly one tier at most.
pub(super) const SOFT_BOTTLENECK_MAX_LIFT: u32 = 200;

/// Adaptive grid sizing rule: small files get a small grid so structure isn't
/// drowned in empty cells; large files get more room so distinct components
/// don't merge.
///
/// Thresholds are expressed in *significant code lines*, not byte length.
/// Byte length is directly gameable — adding a few comment lines lengthens
/// the file and can push it across a threshold, swapping the entire grid
/// (32 → 48 → 64) under the agent without changing any code. Significant
/// lines (comment-only and blank lines filtered out by
/// `count_significant_lines`) approximate "real code size" and don't shift
/// when an agent sprinkles comments. Calibration follows the prior byte
/// thresholds at ~80 bytes/line: 2 KB ≈ 25 lines, 10 KB ≈ 128 lines.
struct GridConfig {
    grid_min: usize,
    grid_mid: usize,
    grid_max: usize,
    small_file_lines: usize,
    medium_file_lines: usize,
}

impl GridConfig {
    const fn default() -> Self {
        Self {
            grid_min: 32,
            grid_mid: 48,
            grid_max: 64,
            small_file_lines: 25,
            medium_file_lines: 128,
        }
    }

    const fn size_for(&self, significant_lines: usize) -> usize {
        if significant_lines <= self.small_file_lines {
            return self.grid_min;
        }
        if significant_lines <= self.medium_file_lines {
            return self.grid_mid;
        }
        self.grid_max
    }
}

const GRID: GridConfig = GridConfig::default();

/// Generation-fraction sample points for population growth classification.
/// Sampling at fractions instead of absolute generation indices lets the same
/// rule work for both fast-test (max=500) and production (max=3000) limits.
const EARLY_SAMPLE_DENOM: u32 = 10;
const LATE_SAMPLE_NUM: u32 = 4;
const LATE_SAMPLE_DEN: u32 = 5;

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

pub(super) fn adaptive_grid_size(significant_lines: usize) -> usize {
    GRID.size_for(significant_lines)
}

enum StepOutcome {
    Continue,
    Extinct,
    Cycled(u32),
}

pub(super) fn simulate_gol(
    encoder: SeedEncoderId,
    initial: &Grid,
    density: f32,
    components: usize,
    rule: Rule,
    max_generations: u32,
) -> EncoderScore {
    let mut canvas = initial.clone();
    let mut history: HashMap<u64, u32> = HashMap::new();
    let mut peak_population = canvas.alive_count() as u32;
    history.insert(canvas.hash(), 0);

    let mut early_pop = 0u32;
    let mut late_pop = 0u32;
    let mut generations_survived = max_generations;
    let mut cycle_period: Option<u32> = None;
    let mut extinct = false;

    for current_gen in 1..=max_generations {
        let (population, outcome) = advance_generation(
            &mut canvas,
            rule,
            &mut history,
            current_gen,
            &mut peak_population,
        );
        if current_gen == max_generations / EARLY_SAMPLE_DENOM {
            early_pop = population;
        }
        if current_gen == max_generations * LATE_SAMPLE_NUM / LATE_SAMPLE_DEN {
            late_pop = population;
        }
        match outcome {
            StepOutcome::Continue => continue,
            StepOutcome::Extinct => {
                generations_survived = current_gen;
                extinct = true;
                break;
            }
            StepOutcome::Cycled(period) => {
                generations_survived = current_gen;
                cycle_period = Some(period);
                late_pop = population;
                break;
            }
        }
    }

    EncoderScore {
        encoder,
        density,
        components,
        generations_survived,
        peak_population,
        cycle_period,
        growth_class: classify_growth(early_pop, late_pop, extinct),
    }
}

fn advance_generation(
    canvas: &mut Grid,
    rule: Rule,
    history: &mut HashMap<u64, u32>,
    current_gen: u32,
    peak_population: &mut u32,
) -> (u32, StepOutcome) {
    *canvas = nit_gol::step::step(canvas, rule, EdgeMode::Dead);
    let population = canvas.alive_count() as u32;
    *peak_population = (*peak_population).max(population);
    if population == 0 {
        return (population, StepOutcome::Extinct);
    }
    let fingerprint = canvas.hash();
    if let Some(&first_seen) = history.get(&fingerprint) {
        return (population, StepOutcome::Cycled(current_gen - first_seen));
    }
    history.insert(fingerprint, current_gen);
    (population, StepOutcome::Continue)
}

fn classify_growth(early_pop: u32, late_pop: u32, extinct: bool) -> GrowthClass {
    if extinct {
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
/// Computed as `1 - (std_dev / mean)` over generations-survived. A high value
/// (≥0.85) means all encoders agree on the file's quality; a low value means
/// the encoders disagree, which usually points at a single weak dimension
/// dragging the report down. Byte-level encoders are excluded — they add
/// noise that doesn't reflect code quality the agent can act on.
pub(super) fn compute_consistency(scores: &[EncoderScore]) -> f32 {
    let samples: Vec<f64> = scores
        .iter()
        .filter(|score| QUALITY_ENCODERS.contains(&score.encoder))
        .map(|score| score.generations_survived as f64)
        .collect();
    if samples.is_empty() {
        return 0.0;
    }
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    if mean == 0.0 {
        return 0.0;
    }
    let std_dev = standard_deviation(&samples, mean);
    (1.0 - (std_dev / mean) as f32).clamp(0.0, 1.0)
}

fn standard_deviation(samples: &[f64], mean: f64) -> f64 {
    let variance = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / samples.len() as f64;
    variance.sqrt()
}
