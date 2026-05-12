//! Genome report engine: structural quality feedback for agent code changes.
//!
//! Runs four quality encoders (three AST-driven + one hybrid) on a source file,
//! simulates Conway's Game of Life on each resulting grid, and produces a
//! [`GenomeReport`] with per-encoder metrics, a composite tier, cross-encoder
//! consistency, and targeted recommendations.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use nit_gol::Rule;

use crate::config::GolSeedSource;
use crate::seed::{encode_seed, SeedEncoderId, SeedInput, SeedParams};

mod format;
mod function_scores;
mod instructions;
mod outlier;
mod parsimony;
mod recommendations;
mod simulation;
mod source_scan;

pub use format::{format_genome_diff, format_genome_report};
use function_scores::{compute_function_scores, surface_top_offender_recommendation};
pub use instructions::GENOME_AGENT_INSTRUCTIONS;
pub use recommendations::generate_recommendations;

/// Percentage of the gap between the worst and second-worst AST encoder used
/// to lift the soft bottleneck. Capped by `simulation::SOFT_BOTTLENECK_MAX_LIFT`.
const SOFT_BOTTLENECK_LIFT_PCT: u32 = 15;

/// Below this threshold a file is trivially small (module re-exports, bare
/// `lib.rs`, etc.) and cannot produce meaningful AST structure. Auto-pass at
/// Tier III so agents are not incentivised to pad small files.
const GENOME_MIN_SIGNIFICANT_LINES: usize = 20;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenomeReport {
    pub file_path: std::path::PathBuf,
    pub encoder_scores: Vec<EncoderScore>,
    pub cross_encoder_consistency: f32,
    pub tier: GenomeTier,
    pub recommendations: Vec<GenomeRecommendation>,
    pub timestamp_ms: u64,
    /// Adaptive grid dimension chosen by `simulation::adaptive_grid_size`.
    pub grid_size: usize,
    /// Parsimony analysis — detects over-engineered code that games genome scores.
    #[serde(default)]
    pub parsimony: ParsimonyInfo,
    /// Per-function structural scores, sorted by `cognitive` descending —
    /// the worst-offender first. Surfaces the *specific* function an
    /// agent or operator should refactor, instead of the file-level
    /// average that today's `tier` averages away. Empty when no
    /// function-like nodes were found (data files, declarations-only
    /// modules).
    #[serde(default)]
    pub function_scores: Vec<FunctionScore>,
}

/// Per-function structural metrics surfaced by the agent-retry prompt:
/// where the function lives, its size, cognitive complexity, peak depth.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FunctionScore {
    pub kind: String,
    pub start_line: u32,
    pub end_line: u32,
    pub node_count: u32,
    pub max_depth: u8,
    /// Cognitive complexity (Sonar): sum of `1 + cf_depth` over every
    /// control-flow node. Penalises nested ladders that plain cyclomatic
    /// treats as linear branch count.
    pub cognitive: u32,
    /// Plain cyclomatic equivalent — count of `RoleBand::ControlFlow`
    /// nodes inside the function. Retained alongside `cognitive` so
    /// callers can compare the two.
    pub cyclomatic: u32,
}

/// Parsimony analysis: measures whether code structure is proportional to purpose.
/// Detects over-split functions, excessive item counts, and comment padding that
/// inflate genome scores without improving actual code quality.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ParsimonyInfo {
    pub fn_count: usize,
    /// Average significant lines per function body (0.0 if no functions).
    pub avg_fn_body_lines: f32,
    /// Top-level items (fn, struct, enum, impl, trait, const, static, type) per
    /// 100 significant lines.
    pub item_density: f32,
    /// Ratio of comment lines to total non-blank lines (0.0–1.0).
    /// Comments include `//`, `///`, `/*`, `*` (doc + block continuation).
    #[serde(default)]
    pub comment_ratio: f32,
    /// Fraction of functions whose body is <= 5 significant lines (0.0–1.0).
    /// A high ratio means most functions are trivially small — likely predicate
    /// extraction or stub duplication rather than meaningful decomposition.
    #[serde(default)]
    pub tiny_fn_fraction: f32,
    /// Count of consecutive-duplicate `//` / `///` comment lines (each repeat
    /// of a non-blank comment after an identical prior comment counts once).
    /// A single duplicate is almost always a merge/refactor accident — it
    /// never carries new information — so any occurrence is flagged.
    #[serde(default)]
    pub duplicate_comment_lines: usize,
    /// `true` when the file shows signs of over-engineering for genome scores.
    /// When set, the tier is capped at Methuselah (IV).
    pub bloat_detected: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EncoderScore {
    pub encoder: SeedEncoderId,
    pub density: f32,
    pub components: usize,
    pub generations_survived: u32,
    pub peak_population: u32,
    pub cycle_period: Option<u32>,
    pub growth_class: GrowthClass,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GrowthClass {
    /// Population expanded over time (early < late) — structurally resilient code.
    Expanding,
    /// Population remained stable — well-balanced code.
    Stable,
    /// Population declined over time (early > late) — structurally fragile code.
    Collapsing,
    /// Population died out completely.
    Extinct,
}

impl GrowthClass {
    pub fn label(self) -> &'static str {
        match self {
            GrowthClass::Expanding => "expanding",
            GrowthClass::Stable => "stable",
            GrowthClass::Collapsing => "collapsing",
            GrowthClass::Extinct => "extinct",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GenomeTier {
    StillLife = 0,
    Oscillator = 1,
    Spaceship = 2,
    Methuselah = 3,
    Replicator = 4,
}

/// Single source of truth for tier boundaries: `(upper_bound_inclusive, tier)`.
/// Hit-tested in order; the last entry is a sentinel matching all higher
/// generation counts.
const TIER_BOUNDARIES: &[(u32, GenomeTier)] = &[
    (50, GenomeTier::StillLife),
    (200, GenomeTier::Oscillator),
    (500, GenomeTier::Spaceship),
    (2000, GenomeTier::Methuselah),
    (u32::MAX, GenomeTier::Replicator),
];

impl GenomeTier {
    pub fn from_generations(g: u32) -> Self {
        for &(upper, tier) in TIER_BOUNDARIES {
            if g <= upper {
                return tier;
            }
        }
        GenomeTier::Replicator
    }

    pub fn numeral(&self) -> &str {
        match self {
            GenomeTier::StillLife => "I",
            GenomeTier::Oscillator => "II",
            GenomeTier::Spaceship => "III",
            GenomeTier::Methuselah => "IV",
            GenomeTier::Replicator => "V",
        }
    }

    pub fn name(&self) -> &str {
        match self {
            GenomeTier::StillLife => "Still Life",
            GenomeTier::Oscillator => "Oscillator",
            GenomeTier::Spaceship => "Spaceship",
            GenomeTier::Methuselah => "Methuselah",
            GenomeTier::Replicator => "Replicator",
        }
    }
}

impl fmt::Display for GenomeTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.numeral(), self.name())
    }
}

/// Per-tier minimum cross-encoder consistency required for the next quality
/// label up. Driven by `quality_level` / `quality_reason`. StillLife has no
/// entry — it always falls through to the "low tier" reason.
const QUALITY_THRESHOLDS: &[(GenomeTier, f32, &str)] = &[
    (GenomeTier::Replicator, 0.85, "Exceptional"),
    (GenomeTier::Methuselah, 0.70, "Excellent"),
    (GenomeTier::Spaceship, 0.50, "Standard"),
    (GenomeTier::Oscillator, 0.25, "Minimum"),
];

impl GenomeReport {
    pub fn quality_level(&self) -> &'static str {
        for &(tier, threshold, label) in QUALITY_THRESHOLDS {
            if self.tier == tier && self.cross_encoder_consistency >= threshold {
                return label;
            }
        }
        "Failing"
    }

    /// Short reason why quality is at its current level. Returns `None` if
    /// quality meets the tier's consistency threshold.
    pub fn quality_reason(&self) -> Option<&'static str> {
        if self.tier == GenomeTier::StillLife {
            return Some("low tier");
        }
        let threshold = QUALITY_THRESHOLDS
            .iter()
            .find(|(tier, _, _)| *tier == self.tier)
            .map(|(_, t, _)| *t)?;
        (self.cross_encoder_consistency < threshold).then_some("low cons")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenomeRecommendation {
    pub metric: String,
    pub severity: RecommendationSeverity,
    pub message: String,
    pub location: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecommendationSeverity {
    Critical,
    Warning,
    Info,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenomeDiff {
    pub file_path: std::path::PathBuf,
    pub tier_before: GenomeTier,
    pub tier_after: GenomeTier,
    pub encoder_diffs: Vec<EncoderDiff>,
    pub consistency_before: f32,
    pub consistency_after: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EncoderDiff {
    pub encoder: SeedEncoderId,
    pub density_delta: f32,
    pub components_delta: i32,
    pub generations_delta: i32,
}

pub fn compute_genome_report(text: &str, file_path: &Path) -> GenomeReport {
    compute_genome_report_inner(text, file_path, simulation::MAX_GENERATIONS)
}

/// Test-only entry point with a reduced generation limit for fast tests.
#[cfg(test)]
pub fn compute_genome_report_fast(text: &str, file_path: &Path) -> GenomeReport {
    compute_genome_report_inner(text, file_path, 500)
}

fn compute_genome_report_inner(text: &str, file_path: &Path, max_generations: u32) -> GenomeReport {
    let significant_lines = count_significant_lines(text, Some(file_path));
    // Parsimony runs FIRST regardless of file size — its job is to catch
    // comment-bloat / duplicate-doc patterns, and those can exist in
    // small files too. Auto-passing a small file shouldn't silently
    // suppress the bloat detector (an agent could dump a file full of
    // duplicate `///` lines and a small body, and we'd miss it).
    let parsimony_info = parsimony::compute_parsimony(text, file_path, significant_lines);
    let function_scores = compute_function_scores(text, file_path);
    if significant_lines < GENOME_MIN_SIGNIFICANT_LINES {
        return small_file_report(
            file_path,
            significant_lines,
            parsimony_info,
            function_scores,
        );
    }

    let grid_size = simulation::adaptive_grid_size(significant_lines);
    let encoder_scores = run_encoders(text, file_path, grid_size, max_generations);
    let cross_encoder_consistency = simulation::compute_consistency(&encoder_scores);

    let tier = compute_tier(&encoder_scores, parsimony_info.bloat_detected);

    let mut recommendations = generate_recommendations(text, file_path, &encoder_scores);
    parsimony::generate_parsimony_recommendations(&parsimony_info, &mut recommendations);
    surface_top_offender_recommendation(&function_scores, &mut recommendations);

    GenomeReport {
        file_path: file_path.to_path_buf(),
        encoder_scores,
        cross_encoder_consistency,
        tier,
        recommendations,
        timestamp_ms: now_millis(),
        grid_size,
        parsimony: parsimony_info,
        function_scores,
    }
}

fn count_significant_lines(text: &str, file_path: Option<&Path>) -> usize {
    // Prefer AST-derived row count when tree-sitter can parse — sprinkling
    // attributes / docs doesn't move this count, so the small-file
    // threshold can't be gamed by padding. The regex fallback only runs
    // on unparseable files (unknown extension / missing grammar).
    if let Some(features) =
        crate::seed::encoders::ast_features::compute_ast_features(text, file_path)
    {
        return features.significant_rows;
    }
    text.lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !source_scan::is_comment_line(trimmed)
        })
        .count()
}

fn run_encoders(
    text: &str,
    file_path: &Path,
    grid_size: usize,
    max_generations: u32,
) -> Vec<EncoderScore> {
    let input = SeedInput {
        text,
        source: GolSeedSource::Editor,
        file_path: Some(file_path),
        version: 0,
    };
    let params = SeedParams::default();
    let conway = Rule::conway();

    simulation::QUALITY_ENCODERS
        .iter()
        .map(|&encoder_id| {
            let encoded = encode_seed(&input, encoder_id, &params, 0, 0, grid_size, grid_size);
            simulation::simulate_gol(
                encoder_id,
                &encoded.grid,
                encoded.stats.density,
                encoded.stats.components,
                conway,
                max_generations,
            )
        })
        .collect()
}

/// The "soft bottleneck" rule replaces the old pure-min approach. Pure min
/// created extreme pressure on the weakest encoder, incentivising agents to
/// over-engineer just to boost one lagging metric. The soft minimum gives a
/// modest lift (capped at `SOFT_BOTTLENECK_MAX_LIFT`) proportional to the gap
/// between the weakest and next-weakest encoder. This means one moderately
/// weak encoder no longer traps the file at a low tier when the others are
/// strong, while genuinely bad structure still scores poorly. The cap also
/// means Replicator (2001+) still requires real quality across all encoders.
fn compute_tier(scores: &[EncoderScore], bloat_detected: bool) -> GenomeTier {
    let mut ast_gens: Vec<u32> = scores
        .iter()
        .filter(|s| simulation::AST_ENCODERS.contains(&s.encoder))
        .map(|s| s.generations_survived)
        .collect();
    ast_gens.sort_unstable();
    let raw_min = ast_gens.first().copied().unwrap_or(0);
    let effective_min = if ast_gens.len() >= 2 {
        let gap = ast_gens[1].saturating_sub(raw_min);
        let lift = (gap * SOFT_BOTTLENECK_LIFT_PCT / 100).min(simulation::SOFT_BOTTLENECK_MAX_LIFT);
        raw_min + lift
    } else {
        raw_min
    };
    let tier = GenomeTier::from_generations(effective_min);
    if bloat_detected && tier > GenomeTier::Methuselah {
        GenomeTier::Methuselah
    } else {
        tier
    }
}

fn small_file_report(
    file_path: &Path,
    significant_lines: usize,
    parsimony: ParsimonyInfo,
    function_scores: Vec<FunctionScore>,
) -> GenomeReport {
    // Bloat (e.g., duplicate doc comments) still caps the auto-pass tier
    // even for small files — otherwise stuffing a 10-line file with
    // duplicate `///` blocks would silently slip through.
    let tier = if parsimony.bloat_detected {
        GenomeTier::Methuselah
    } else {
        GenomeTier::Spaceship
    };
    let mut recommendations = vec![GenomeRecommendation {
        metric: "file_size".into(),
        severity: RecommendationSeverity::Info,
        message: format!(
            "Trivial file ({significant_lines} significant lines < {GENOME_MIN_SIGNIFICANT_LINES}): auto-pass. Do not pad small files to boost scores."
        ),
        location: None,
    }];
    // Surface parsimony-derived recommendations (duplicate comments,
    // comment-bloat ratio, over-split functions). The encoder path won't
    // run on small files, but the parsimony detector did — its findings
    // should still reach the operator.
    parsimony::generate_parsimony_recommendations(&parsimony, &mut recommendations);
    GenomeReport {
        file_path: file_path.to_path_buf(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 1.0,
        tier,
        recommendations,
        timestamp_ms: now_millis(),
        grid_size: 0,
        parsimony,
        function_scores,
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn compute_genome_diff(before: &GenomeReport, after: &GenomeReport) -> GenomeDiff {
    let encoder_diffs = after
        .encoder_scores
        .iter()
        .map(|after_score| diff_encoder(before, after_score))
        .collect();

    GenomeDiff {
        file_path: after.file_path.clone(),
        tier_before: before.tier,
        tier_after: after.tier,
        encoder_diffs,
        consistency_before: before.cross_encoder_consistency,
        consistency_after: after.cross_encoder_consistency,
    }
}

fn diff_encoder(before: &GenomeReport, after_score: &EncoderScore) -> EncoderDiff {
    let before_score = before
        .encoder_scores
        .iter()
        .find(|s| s.encoder == after_score.encoder);
    let (density_delta, components_delta, generations_delta) = match before_score {
        Some(bs) => (
            after_score.density - bs.density,
            after_score.components as i32 - bs.components as i32,
            after_score.generations_survived as i32 - bs.generations_survived as i32,
        ),
        None => (
            after_score.density,
            after_score.components as i32,
            after_score.generations_survived as i32,
        ),
    };
    EncoderDiff {
        encoder: after_score.encoder,
        density_delta,
        components_delta,
        generations_delta,
    }
}

#[cfg(test)]
#[path = "tests/genome_report.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/genome_check.rs"]
mod genome_check;

#[cfg(test)]
#[path = "tests/genome_function_scores.rs"]
mod function_scores_tests;

#[cfg(test)]
#[path = "tests/genome_parsimony.rs"]
mod parsimony_tests;

#[cfg(test)]
#[path = "tests/genome_adversarial.rs"]
mod adversarial;

#[cfg(test)]
#[path = "tests/genome_proptest.rs"]
mod proptest;
