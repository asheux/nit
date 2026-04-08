//! Genome report engine: structural quality feedback for agent code changes.
//!
//! Runs four quality encoders (three AST-driven + one hybrid) on a source file,
//! simulates Conway's Game of Life on each resulting grid, and produces a
//! [`GenomeReport`] with per-encoder metrics, a composite tier, cross-encoder
//! consistency, and targeted recommendations.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tree_sitter::{Parser, Tree};

use nit_gol::{EdgeMode, Grid, Rule};

use crate::config::GolSeedSource;
use crate::seed::{encode_seed, SeedEncoderId, SeedInput, SeedParams};

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenomeReport {
    pub file_path: std::path::PathBuf,
    pub encoder_scores: Vec<EncoderScore>,
    pub cross_encoder_consistency: f32,
    pub tier: GenomeTier,
    pub recommendations: Vec<GenomeRecommendation>,
    pub timestamp_ms: u64,
    /// Grid dimension used for this report (adaptive: 32, 48, or 64).
    pub grid_size: usize,
    /// Parsimony analysis — detects over-engineered code that games genome scores.
    #[serde(default)]
    pub parsimony: ParsimonyInfo,
}

/// Parsimony analysis: measures whether code structure is proportional to purpose.
/// Detects over-split functions, excessive item counts, and comment padding that
/// inflate genome scores without improving actual code quality.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParsimonyInfo {
    /// Number of function/method bodies found in the file.
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
    /// `true` when the file shows signs of over-engineering for genome scores.
    /// When set, the tier is capped at Methuselah (IV).
    pub bloat_detected: bool,
}

impl Default for ParsimonyInfo {
    fn default() -> Self {
        Self {
            fn_count: 0,
            avg_fn_body_lines: 0.0,
            item_density: 0.0,
            comment_ratio: 0.0,
            tiny_fn_fraction: 0.0,
            bloat_detected: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EncoderScore {
    pub encoder: SeedEncoderId,
    pub density: f32,
    pub components: usize,
    pub generations_survived: u32,
    pub peak_population: u32,
    pub cycle_period: Option<u32>,
    /// Population trajectory classification based on growth curve analysis.
    pub growth_class: GrowthClass,
}

/// Classification of population trajectory during GoL simulation.
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

impl GenomeTier {
    pub fn from_generations(g: u32) -> Self {
        match g {
            0..=50 => GenomeTier::StillLife,
            51..=200 => GenomeTier::Oscillator,
            201..=500 => GenomeTier::Spaceship,
            501..=2000 => GenomeTier::Methuselah,
            _ => GenomeTier::Replicator,
        }
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

impl GenomeReport {
    /// Classify overall quality from tier and consistency.
    pub fn quality_level(&self) -> &'static str {
        match (self.tier, self.cross_encoder_consistency) {
            (GenomeTier::Replicator, c) if c >= 0.85 => "Exceptional",
            (GenomeTier::Methuselah, c) if c >= 0.70 => "Excellent",
            (GenomeTier::Spaceship, c) if c >= 0.50 => "Standard",
            (GenomeTier::Oscillator, c) if c >= 0.25 => "Minimum",
            _ => "Failing",
        }
    }

    /// Short reason why quality is at its current level.
    /// Returns `None` if quality meets the tier's consistency threshold.
    pub fn quality_reason(&self) -> Option<&'static str> {
        let (needed_tier, needed_c) = match self.tier {
            GenomeTier::Replicator => (GenomeTier::Replicator, 0.85),
            GenomeTier::Methuselah => (GenomeTier::Methuselah, 0.70),
            GenomeTier::Spaceship => (GenomeTier::Spaceship, 0.50),
            GenomeTier::Oscillator => (GenomeTier::Oscillator, 0.25),
            GenomeTier::StillLife => return Some("low tier"),
        };
        if self.tier >= needed_tier && self.cross_encoder_consistency < needed_c {
            Some("low cons")
        } else {
            None
        }
    }
}

pub const GENOME_AGENT_INSTRUCTIONS: &str = "\
MISSION — nit coding lab\n\
You are an agent in nit, an agentic coding lab that measures structural code \
quality by encoding source files as Game of Life genomes. nit's goal is to \
produce superprogrammer agents — agents that write naturally well-structured \
code. The highest tier is Replicator (Tier V, 2001+ generations). Your \
aspiration is Tier V, but never at the cost of over-engineering. Write code \
that is good because it solves the problem well, not because it games a metric.\n\
\n\
TIER LADDER (your progression path):\n\
  I   Still Life   (0-50 gen)     — Failing. Code does not survive.\n\
  II  Oscillator   (51-200 gen)   — Minimum. Fragile structure.\n\
  III Spaceship    (201-500 gen)  — Standard. Acceptable baseline.\n\
  IV  Methuselah   (501-2000 gen) — Excellent. Strong architecture.\n\
  V   Replicator   (2001+ gen)    — Exceptional. Elite code genome.\n\
Your minimum target is Tier III. Consistent quality at your current tier \
will naturally elevate your threshold. Falling below your threshold triggers \
automatic retries.\n\
\n\
EQUILIBRIUM RULE — quality without bloat:\n\
nit enforces a parsimony check on every evaluation. Code that is \
over-engineered — many trivially small functions, unnecessary type \
declarations, or artificial structural variety added solely to inflate \
genome scores — is detected and penalized. When parsimony bloat is \
detected, the tier is capped at Methuselah (IV) regardless of how well the \
GoL simulation performs. The right approach:\n\
  - Write the simplest correct solution first.\n\
  - If nit reports low quality, improve structure where it naturally helps \
readability and maintainability.\n\
  - Do NOT split a clear 15-line function into five 3-line functions.\n\
  - Do NOT extract trivial predicates into their own functions. Inline \
simple boolean checks — a 3-line function that just calls `.any()` or \
checks two conditions is not a meaningful abstraction.\n\
  - Do NOT copy-paste function bodies to create stubs or near-identical \
variants. Use macros or generics for repetitive patterns.\n\
  - Do NOT add enums, structs, or traits that serve no functional purpose.\n\
  - Do NOT add comments to boost scores. Comments must explain non-obvious \
logic only. Restating what code does (\"// increment counter\"), adding \
doc comments on trivial private helpers, or inserting section markers \
purely for token diversity is detected as comment padding and penalized.\n\
  - Do NOT vary function signatures (generic bounds, error styles) purely \
for token diversity.\n\
  - Files with >40% comment lines are flagged and tier-capped automatically.\n\
  - Files where >50% of functions have <= 5 lines are flagged and \
tier-capped automatically.\n\
Good code naturally scores well. Over-engineered code is caught and penalized.\n\
\n\
HOW YOU ARE MEASURED:\n\
Your code is evaluated across four encoders. Each captures a different \
dimension of code quality. Cross-encoder consistency measures how much they \
agree — low consistency means some dimensions are strong but others are weak. \
Your tier is determined by a soft bottleneck of the AST-driven encoders — \
the weakest encoder matters most, but strong performance on other encoders \
provides a modest lift. Focus on balanced, natural code rather than \
obsessing over one encoder.\n\
\n\
ENCODER GUIDE (what each measures → how to improve naturally):\n\
\n\
AST-driven encoders (determine the overall tier):\n\
  token_spectrum — token semantic role distribution (keywords, operators, \
identifiers, literals, comments).\n\
    → Write code with natural variety. Avoid long repetitive blocks of \
similar tokens. Do NOT add comments to boost this encoder — comments \
that exist only for score inflation are detected and penalized by the \
parsimony system.\n\
  ast_structure — syntactic tree shape (nesting depth, branching factor, span \
size, node type variety).\n\
    → Use appropriate abstraction boundaries. Reduce deep nesting with early \
returns. A mix of types (structs, enums, fns) emerges naturally from \
good design — do not add types just for variety.\n\
  complexity_field — spatial heatmap of cyclomatic complexity, nesting depth, \
token entropy, and identifier uniqueness.\n\
    → Keep cyclomatic complexity reasonable per function (aim for <= 8). \
Use descriptive names. Distribute logic across well-motivated functions.\n\
\n\
Hybrid encoder (AST-aware, whitespace-filtered):\n\
  structural — operates on semantic token roles from tree-sitter.\n\
    → Naturally varied code scores well. Different function shapes emerge \
from solving different sub-problems — not from artificially varying \
signatures or padding with comments.\n\
\n\
TARGETS (guidelines, not hard requirements to engineer toward):\n\
- Tier III+ (Spaceship) on all AST encoders.\n\
- Cyclomatic complexity <= 8 per function.\n\
- Nesting depth <= 3 on average.\n\
- Cross-encoder consistency >= 0.50.\n\
These are outcomes of good code, not specifications to engineer toward.\n\
\n\
nit measures quality automatically after your changes are written to disk. \
Do NOT call [evaluate_genome] — nit evaluates externally and will retry your \
turn with specific feedback if quality degrades. Focus on writing good code; \
if tier drops below III, nit will tell you exactly what to fix.\n\
\n\
SMALL FILES: Files with fewer than 20 significant lines (lib.rs, mod.rs, \
re-export files) receive an automatic Tier III pass. Do NOT pad these files \
with unnecessary code, enums, helpers, or doc comments just to boost genome \
scores. Keep small files minimal and clean.";

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

// ---------------------------------------------------------------------------
// Simulation constants
// ---------------------------------------------------------------------------

const MAX_GENERATIONS: u32 = 3000;

/// Maximum lift (in generations) the soft bottleneck rule can apply.
/// Caps at 200 so Replicator (2001+) still requires genuine quality across all
/// encoders — the lift can bump you up roughly one tier at most.
const SOFT_BOTTLENECK_MAX_LIFT: u32 = 200;

/// Minimum grid dimension (small files).
const GRID_MIN: usize = 32;
/// Maximum grid dimension (cap for very large files — diminishing returns beyond this).
const GRID_MAX: usize = 64;

/// Compute adaptive grid size based on file byte length.
/// Scales from 32x32 for small files up to 64x64 for large files,
/// preserving structural fidelity without blowing up simulation cost.
fn adaptive_grid_size(file_bytes: usize) -> usize {
    match file_bytes {
        0..=2048 => GRID_MIN, // <= 2KB: 32x32 (1,024 cells)
        2049..=10240 => 48,   // 2-10KB: 48x48 (2,304 cells)
        _ => GRID_MAX,        // > 10KB: 64x64 (4,096 cells)
    }
}

// ---------------------------------------------------------------------------
// Core computation
// ---------------------------------------------------------------------------

/// AST-driven encoder IDs used for tier determination.
const AST_ENCODERS: [SeedEncoderId; 3] = [
    SeedEncoderId::TokenSpectrum,
    SeedEncoderId::AstStructure,
    SeedEncoderId::ComplexityField,
];

/// The four encoders used for code quality measurement.
/// Three AST-driven (determine tier) + one hybrid (structural).
/// Byte-level encoders (ascii_bytes, hilbert_bits, lifehash16) are excluded
/// entirely — they measure surface byte patterns that add noise to quality
/// signals and cannot be meaningfully improved through code changes.
const QUALITY_ENCODERS: [SeedEncoderId; 4] = [
    SeedEncoderId::TokenSpectrum,
    SeedEncoderId::AstStructure,
    SeedEncoderId::ComplexityField,
    SeedEncoderId::Structural,
];

/// Compute a full genome report for the given source text and file path.
/// Minimum number of non-blank, non-comment lines for a file to be evaluated by
/// the genome encoders.  Files below this threshold are trivially small (module
/// re-exports, bare `lib.rs`, etc.) and cannot produce meaningful AST structure.
/// They receive an automatic Tier III pass so agents are not incentivised to pad
/// them with unnecessary code.
const GENOME_MIN_SIGNIFICANT_LINES: usize = 20;

pub fn compute_genome_report(text: &str, file_path: &Path) -> GenomeReport {
    let significant_lines = text
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with("//")
                && !trimmed.starts_with("/*")
                && !trimmed.starts_with('*')
        })
        .count();

    if significant_lines < GENOME_MIN_SIGNIFICANT_LINES {
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        return GenomeReport {
            file_path: file_path.to_path_buf(),
            encoder_scores: Vec::new(),
            cross_encoder_consistency: 1.0,
            tier: GenomeTier::Spaceship,
            recommendations: vec![GenomeRecommendation {
                metric: "file_size".into(),
                severity: RecommendationSeverity::Info,
                message: format!(
                    "Trivial file ({significant_lines} significant lines < {GENOME_MIN_SIGNIFICANT_LINES}): auto-pass. Do not pad small files to boost scores."
                ),
                location: None,
            }],
            timestamp_ms,
            grid_size: 0,
            parsimony: ParsimonyInfo::default(),
        };
    }

    let input = SeedInput {
        text,
        source: GolSeedSource::Editor,
        file_path: Some(file_path),
        version: 0,
    };
    let params = SeedParams::default();
    let conway = Rule::conway();
    let grid_size = adaptive_grid_size(text.len());

    // Encode and simulate only the 4 quality encoders (3 AST + 1 hybrid).
    // Byte-level encoders (ascii_bytes, hilbert_bits, lifehash16) are not
    // computed — they add noise to quality measurement.
    let mut encoder_scores = Vec::with_capacity(QUALITY_ENCODERS.len());
    for &encoder_id in &QUALITY_ENCODERS {
        let encoded = encode_seed(&input, encoder_id, &params, 0, 0, grid_size, grid_size);
        let score = simulate_gol(
            encoder_id,
            &encoded.grid,
            encoded.stats.density,
            encoded.stats.components,
            conway,
        );
        encoder_scores.push(score);
    }

    // Step 5: cross-encoder consistency.
    let cross_encoder_consistency = compute_consistency(&encoder_scores);

    // Step 6: tier from soft minimum of AST-driven encoders.
    //
    // The "soft bottleneck" rule replaces the old pure-min approach.  Pure min
    // created extreme pressure on the weakest encoder, incentivising agents to
    // over-engineer just to boost one lagging metric.  The soft minimum gives a
    // modest lift (capped at 200 generations) proportional to the gap between
    // the weakest and next-weakest encoder.  This means one moderately weak
    // encoder no longer traps the file at a low tier when the others are strong,
    // while genuinely bad structure still scores poorly.  The 200-gen cap also
    // means Replicator (2001+) still requires real quality across all encoders.
    let mut ast_gens: Vec<u32> = encoder_scores
        .iter()
        .filter(|s| AST_ENCODERS.contains(&s.encoder))
        .map(|s| s.generations_survived)
        .collect();
    ast_gens.sort_unstable();
    let raw_min = ast_gens.first().copied().unwrap_or(0);
    let effective_min = if ast_gens.len() >= 2 {
        let next = ast_gens[1];
        let gap = next.saturating_sub(raw_min);
        let lift = (gap * 15 / 100).min(SOFT_BOTTLENECK_MAX_LIFT);
        raw_min + lift
    } else {
        raw_min
    };
    let mut tier = GenomeTier::from_generations(effective_min);

    // Step 7: parsimony analysis — detect over-engineered code.
    let parsimony = compute_parsimony(text, file_path, significant_lines);
    if parsimony.bloat_detected && tier > GenomeTier::Methuselah {
        tier = GenomeTier::Methuselah;
    }

    // Step 8: recommendations.
    let mut recommendations = generate_recommendations(text, file_path, &encoder_scores);
    generate_parsimony_recommendations(&parsimony, &mut recommendations);

    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    GenomeReport {
        file_path: file_path.to_path_buf(),
        encoder_scores,
        cross_encoder_consistency,
        tier,
        recommendations,
        timestamp_ms,
        grid_size,
        parsimony,
    }
}

/// Simulate Conway's Game of Life on the grid for up to `MAX_GENERATIONS` generations.
/// Returns an `EncoderScore` with simulation results.
fn simulate_gol(
    encoder: SeedEncoderId,
    initial: &Grid,
    density: f32,
    components: usize,
    rule: Rule,
) -> EncoderScore {
    let mut grid = initial.clone();
    let mut seen: HashMap<u64, u32> = HashMap::new();
    let mut peak_population: u32 = grid.alive_count() as u32;
    let hash0 = grid.hash();
    seen.insert(hash0, 0);

    let mut generations_survived: u32 = 0;
    let mut cycle_period: Option<u32> = None;
    // Track population at early and late stages for growth classification.
    let mut early_pop: u32 = 0;
    let mut late_pop: u32 = 0;
    let mut died = false;

    for gen in 1..=MAX_GENERATIONS {
        grid = nit_gol::step::step(&grid, rule, EdgeMode::Dead);
        let alive = grid.alive_count() as u32;
        if alive > peak_population {
            peak_population = alive;
        }
        // Sample early population (around 10% of sim).
        if gen == MAX_GENERATIONS / 10 {
            early_pop = alive;
        }
        // Sample late population (around 80% of sim).
        if gen == MAX_GENERATIONS * 4 / 5 {
            late_pop = alive;
        }
        // All cells dead — no further evolution possible.
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
        generations_survived = MAX_GENERATIONS;
    }

    let growth_class = if died {
        GrowthClass::Extinct
    } else if early_pop == 0 || late_pop == 0 {
        // Not enough data — treat as stable.
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

/// Compute cross-encoder consistency from actionable encoders only (AST + structural).
/// Byte-level encoders are excluded — they add noise that doesn't reflect
/// code quality the agent can act on.
fn compute_consistency(scores: &[EncoderScore]) -> f32 {
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

// ---------------------------------------------------------------------------
// Parsimony analysis
// ---------------------------------------------------------------------------

/// Minimum significant lines before parsimony analysis is applied.
/// Below this threshold, files are too small for meaningful bloat detection.
const PARSIMONY_MIN_LINES: usize = 40;

/// Maximum average function body lines before bloat is flagged.
/// Functions averaging fewer than this many significant lines, combined with
/// a high function count, indicate over-splitting.
const PARSIMONY_AVG_FN_BODY_THRESHOLD: f32 = 3.0;

/// Minimum function count before bloat can be flagged.
/// Files with fewer functions than this are not considered over-split regardless
/// of average body size.
const PARSIMONY_MIN_FN_COUNT: usize = 15;

/// A function body with this many significant lines or fewer is considered
/// "tiny" for the tiny-function-fraction check.
const PARSIMONY_TINY_FN_LINES: usize = 5;

/// If more than this fraction of functions are tiny (body <= 5 sig lines),
/// the file is flagged for predicate over-extraction / stub duplication.
/// Requires at least 10 functions to avoid false positives on small files.
const PARSIMONY_TINY_FN_FRACTION_THRESHOLD: f32 = 0.50;

/// Minimum function count for the tiny-function-fraction check.
/// Set at 12 to avoid false positives on small structs with one-liner
/// accessor methods (a struct with 10 methods is normal; 12+ tiny functions
/// in a single file suggests predicate over-extraction).
const PARSIMONY_TINY_FN_MIN_COUNT: usize = 12;

/// Maximum comment-to-code ratio before comment padding is flagged.
/// Well-documented code typically sits at 15-25%.  Above 40% suggests comments
/// are being added to diversify the token stream for genome scores rather than
/// to explain non-obvious logic.
const PARSIMONY_COMMENT_RATIO_THRESHOLD: f32 = 0.40;

/// Compute parsimony metrics from the source text using tree-sitter AST analysis.
fn compute_parsimony(text: &str, file_path: &Path, significant_lines: usize) -> ParsimonyInfo {
    let tree = match ts_parse(text, file_path) {
        Some(t) => t,
        None => return ParsimonyInfo::default(),
    };

    let root = tree.root_node();
    let mut fn_body_sizes: Vec<usize> = Vec::new();
    let mut top_level_items: usize = 0;

    count_items_recursive(&root, text, 0, &mut fn_body_sizes, &mut top_level_items);

    let fn_count = fn_body_sizes.len();
    let fn_body_lines_total: usize = fn_body_sizes.iter().sum();
    let avg_fn_body_lines = if fn_count > 0 {
        fn_body_lines_total as f32 / fn_count as f32
    } else {
        0.0
    };
    let tiny_fn_fraction = if fn_count > 0 {
        let tiny = fn_body_sizes
            .iter()
            .filter(|&&s| s <= PARSIMONY_TINY_FN_LINES)
            .count();
        tiny as f32 / fn_count as f32
    } else {
        0.0
    };

    let item_density = if significant_lines > 0 {
        top_level_items as f32 / significant_lines as f32 * 100.0
    } else {
        0.0
    };

    // Comment ratio: comment lines / total non-blank lines.
    let mut comment_lines: usize = 0;
    let mut non_blank_lines: usize = 0;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        non_blank_lines += 1;
        if t.starts_with("//") || t.starts_with("///") || t.starts_with("/*") || t.starts_with('*')
        {
            comment_lines += 1;
        }
    }
    let comment_ratio = if non_blank_lines > 0 {
        comment_lines as f32 / non_blank_lines as f32
    } else {
        0.0
    };

    // Bloat detection: either over-split functions OR comment padding.
    let over_split = significant_lines >= PARSIMONY_MIN_LINES
        && fn_count >= PARSIMONY_MIN_FN_COUNT
        && avg_fn_body_lines > 0.0
        && avg_fn_body_lines < PARSIMONY_AVG_FN_BODY_THRESHOLD;

    // For comment padding, use non_blank_lines (not significant_lines) as the
    // minimum — the whole point of comment padding is that it inflates total
    // lines while keeping significant (code-only) lines low.
    let comment_padded =
        non_blank_lines >= PARSIMONY_MIN_LINES && comment_ratio > PARSIMONY_COMMENT_RATIO_THRESHOLD;

    // Tiny-function-fraction: catches predicate over-extraction and stub
    // duplication even when the average body size is pulled up by a few
    // larger functions (e.g. 10 two-liners + 2 fifty-liners → avg ~12,
    // but 83% of functions are tiny).
    let too_many_tiny = fn_count >= PARSIMONY_TINY_FN_MIN_COUNT
        && tiny_fn_fraction > PARSIMONY_TINY_FN_FRACTION_THRESHOLD;

    let bloat_detected = over_split || comment_padded || too_many_tiny;

    ParsimonyInfo {
        fn_count,
        avg_fn_body_lines,
        item_density,
        comment_ratio,
        tiny_fn_fraction,
        bloat_detected,
    }
}

/// Recursively count function bodies and top-level items in the AST.
/// Each function's significant body line count is pushed to `fn_body_sizes`.
fn count_items_recursive(
    node: &tree_sitter::Node<'_>,
    text: &str,
    depth: usize,
    fn_body_sizes: &mut Vec<usize>,
    top_level_items: &mut usize,
) {
    let kind = node.kind();

    // Count top-level items (depth 0 or inside impl/trait at depth 1).
    let is_item = matches!(
        kind,
        "function_item"
            | "function_definition"
            | "function_declaration"
            | "struct_item"
            | "enum_item"
            | "type_item"
            | "trait_item"
            | "impl_item"
            | "const_item"
            | "static_item"
            | "class_definition"
            | "decorated_definition"
    );

    if is_item && depth <= 1 {
        *top_level_items += 1;
    }

    // Count function bodies and their significant line counts.
    let is_fn = matches!(
        kind,
        "function_item"
            | "function_definition"
            | "function_declaration"
            | "method_definition"
            | "arrow_function"
    );

    if is_fn {
        let start = node.start_position().row;
        let end = node.end_position().row;
        // Count significant lines within the function span.
        let body_sig_lines = text
            .lines()
            .skip(start)
            .take(end.saturating_sub(start) + 1)
            .filter(|line| {
                let t = line.trim();
                !t.is_empty()
                    && !t.starts_with("//")
                    && !t.starts_with("/*")
                    && !t.starts_with('*')
                    && !t.starts_with("///")
                    && t != "{"
                    && t != "}"
            })
            .count();
        fn_body_sizes.push(body_sig_lines);
    }

    // Recurse into children. For impl/trait blocks, increment depth so their
    // inner items are counted at depth 1 (still top-level conceptually).
    let child_depth = if matches!(kind, "impl_item" | "trait_item" | "class_definition") {
        depth + 1
    } else {
        depth
    };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        // Don't double-count: skip nested function definitions inside functions
        // (closures are fine — they don't match our function kinds).
        if is_fn
            && matches!(
                child.kind(),
                "function_item" | "function_definition" | "function_declaration"
            )
        {
            continue;
        }
        count_items_recursive(&child, text, child_depth, fn_body_sizes, top_level_items);
    }
}

/// Add parsimony-related recommendations when bloat is detected.
fn generate_parsimony_recommendations(
    parsimony: &ParsimonyInfo,
    recs: &mut Vec<GenomeRecommendation>,
) {
    // Over-split functions.
    let over_split = parsimony.fn_count >= PARSIMONY_MIN_FN_COUNT
        && parsimony.avg_fn_body_lines > 0.0
        && parsimony.avg_fn_body_lines < PARSIMONY_AVG_FN_BODY_THRESHOLD;

    if over_split {
        recs.push(GenomeRecommendation {
            metric: "parsimony".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Over-engineered: {} functions averaging {:.1} lines each. \
                 Tier capped at IV (Methuselah). Consolidate trivially small \
                 functions — merge related logic instead of splitting into \
                 many tiny functions to inflate genome scores.",
                parsimony.fn_count, parsimony.avg_fn_body_lines,
            ),
            location: None,
        });
    } else if parsimony.fn_count >= 10
        && parsimony.avg_fn_body_lines > 0.0
        && parsimony.avg_fn_body_lines < PARSIMONY_AVG_FN_BODY_THRESHOLD
    {
        // Soft warning even below the hard threshold.
        recs.push(GenomeRecommendation {
            metric: "parsimony".into(),
            severity: RecommendationSeverity::Info,
            message: format!(
                "Functions average {:.1} lines. Consider whether some can be \
                 consolidated — small focused functions are good, but over-splitting \
                 simple logic adds complexity without improving quality.",
                parsimony.avg_fn_body_lines,
            ),
            location: None,
        });
    }

    // Comment padding.
    if parsimony.comment_ratio > PARSIMONY_COMMENT_RATIO_THRESHOLD {
        recs.push(GenomeRecommendation {
            metric: "comment_padding".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Comment padding detected: {:.0}% of non-blank lines are comments. \
                 Tier capped at IV (Methuselah). Comments improve genome token \
                 diversity scores but adding them to game the system is penalized. \
                 Keep doc comments on public API items; remove trivial or redundant \
                 comments on private helpers and obvious logic.",
                parsimony.comment_ratio * 100.0,
            ),
            location: None,
        });
    } else if parsimony.comment_ratio > 0.30 {
        recs.push(GenomeRecommendation {
            metric: "comment_padding".into(),
            severity: RecommendationSeverity::Info,
            message: format!(
                "Comment ratio is {:.0}%. Approaching the 40% parsimony threshold. \
                 Ensure comments explain non-obvious logic rather than restating code.",
                parsimony.comment_ratio * 100.0,
            ),
            location: None,
        });
    }

    // Tiny-function-fraction: predicate over-extraction / stub duplication.
    let too_many_tiny = parsimony.fn_count >= PARSIMONY_TINY_FN_MIN_COUNT
        && parsimony.tiny_fn_fraction > PARSIMONY_TINY_FN_FRACTION_THRESHOLD;

    if too_many_tiny {
        let tiny_count = (parsimony.tiny_fn_fraction * parsimony.fn_count as f32).round() as usize;
        recs.push(GenomeRecommendation {
            metric: "tiny_functions".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Predicate over-extraction: {tiny_count} of {} functions have \
                 <= {PARSIMONY_TINY_FN_LINES} significant lines ({:.0}%). Tier capped \
                 at IV (Methuselah). Inline trivial predicates, combine related \
                 checks into single functions, and use macros for repetitive stubs \
                 instead of copy-pasting function bodies.",
                parsimony.fn_count,
                parsimony.tiny_fn_fraction * 100.0,
            ),
            location: None,
        });
    }
}

// ---------------------------------------------------------------------------
// Diff computation
// ---------------------------------------------------------------------------

pub fn compute_genome_diff(before: &GenomeReport, after: &GenomeReport) -> GenomeDiff {
    let mut encoder_diffs = Vec::new();
    for after_score in &after.encoder_scores {
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
        encoder_diffs.push(EncoderDiff {
            encoder: after_score.encoder,
            density_delta,
            components_delta,
            generations_delta,
        });
    }

    GenomeDiff {
        file_path: after.file_path.clone(),
        tier_before: before.tier,
        tier_after: after.tier,
        encoder_diffs,
        consistency_before: before.cross_encoder_consistency,
        consistency_after: after.cross_encoder_consistency,
    }
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

pub fn format_genome_report(report: &GenomeReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("[genome report] {}\n", report.file_path.display()));
    out.push_str(&format!(
        "Quality: {} (tier {}, consistency {:.2})\n",
        report.quality_level(),
        report.tier.numeral(),
        report.cross_encoder_consistency
    ));
    out.push_str(&format!(
        "Tier: {} ({}) [grid {}x{}]\n",
        report.tier.numeral(),
        report.tier.name(),
        report.grid_size,
        report.grid_size,
    ));
    out.push_str(&format!(
        "Cross-encoder consistency: {:.2}\n",
        report.cross_encoder_consistency
    ));
    if report.parsimony.fn_count > 0 || report.parsimony.comment_ratio > 0.0 {
        out.push_str(&format!(
            "Parsimony: {} fns, avg {:.1} lines/fn, {:.0}% tiny, {:.0}% comments{}\n",
            report.parsimony.fn_count,
            report.parsimony.avg_fn_body_lines,
            report.parsimony.tiny_fn_fraction * 100.0,
            report.parsimony.comment_ratio * 100.0,
            if report.parsimony.bloat_detected {
                " [BLOAT — tier capped]"
            } else {
                ""
            },
        ));
    }
    out.push('\n');

    out.push_str("Encoder scores:\n");
    for score in &report.encoder_scores {
        out.push_str(&format!(
            "  {}: density={:.2}, components={}, generations={}, peak_pop={}, growth={}{}\n",
            score.encoder.label(),
            score.density,
            score.components,
            score.generations_survived,
            score.peak_population,
            score.growth_class.label(),
            match score.cycle_period {
                Some(p) => format!(", cycle={p}"),
                None => String::new(),
            },
        ));
    }

    if !report.recommendations.is_empty() {
        out.push_str("\nRecommendations:\n");
        for rec in &report.recommendations {
            let sev = match rec.severity {
                RecommendationSeverity::Critical => "CRITICAL",
                RecommendationSeverity::Warning => "WARNING",
                RecommendationSeverity::Info => "INFO",
            };
            out.push_str(&format!("  [{sev}] {}\n", rec.message));
        }
    }
    out
}

pub fn format_genome_diff(diff: &GenomeDiff) -> String {
    let mut out = String::new();
    out.push_str(&format!("[genome diff] {}\n", diff.file_path.display()));

    let tier_arrow = if diff.tier_after > diff.tier_before {
        "upgraded"
    } else if diff.tier_after < diff.tier_before {
        "regressed"
    } else {
        "unchanged"
    };
    out.push_str(&format!(
        "Tier: {} -> {} ({})\n",
        diff.tier_before.numeral(),
        diff.tier_after.numeral(),
        tier_arrow,
    ));

    let consistency_delta = diff.consistency_after - diff.consistency_before;
    out.push_str(&format!(
        "Consistency: {:.2} -> {:.2} ({:+.2})\n\n",
        diff.consistency_before, diff.consistency_after, consistency_delta,
    ));

    out.push_str(&format!(
        "{:<20} {:>10} {:>10} {:>10}\n",
        "Encoder", "Density", "Components", "Generations"
    ));
    for ed in &diff.encoder_diffs {
        out.push_str(&format!(
            "{:<20} {:>+10.2} {:>+10} {:>+10}\n",
            ed.encoder.label(),
            ed.density_delta,
            ed.components_delta,
            ed.generations_delta,
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Phase 6: Recommendation engine
// ---------------------------------------------------------------------------

/// Detect language from file extension and parse with tree-sitter.
fn ts_parse(text: &str, file_path: &Path) -> Option<Tree> {
    let ext = file_path.extension()?.to_str()?;
    let language = match ext {
        "rs" => tree_sitter_rust::language(),
        "py" => tree_sitter_python::language(),
        "js" | "jsx" | "mjs" | "cjs" => tree_sitter_javascript::language(),
        "ts" | "tsx" => tree_sitter_typescript::language_typescript(),
        "html" | "htm" => tree_sitter_html::language(),
        "css" => tree_sitter_css::language(),
        "json" => tree_sitter_json::language(),
        "toml" => tree_sitter_toml::language(),
        "sh" | "bash" => tree_sitter_bash::language(),
        _ => return None,
    };
    let mut parser = Parser::new();
    parser.set_language(language).ok()?;
    parser.parse(text, None)
}

/// Generate targeted, actionable recommendations from encoder scores and AST analysis.
pub fn generate_recommendations(
    text: &str,
    file_path: &Path,
    scores: &[EncoderScore],
) -> Vec<GenomeRecommendation> {
    let mut recs = Vec::new();

    // Density recommendations (no tree-sitter needed).
    // Low density means poor structural variety; high density is good.
    for score in scores {
        if matches!(
            score.encoder,
            SeedEncoderId::TokenSpectrum
                | SeedEncoderId::AstStructure
                | SeedEncoderId::ComplexityField
        ) && score.density < 0.15
        {
            recs.push(GenomeRecommendation {
                metric: "density".into(),
                severity: RecommendationSeverity::Warning,
                message: format!(
                    "{} density is {:.2}. The token-role distribution lacks variety. \
                     Mix different token types: keywords, operators, identifiers, types, and literals. \
                     Break up uniform code blocks with varied function shapes.",
                    score.encoder.label(),
                    score.density,
                ),
                location: None,
            });
        }
    }

    if let Some(ast_score) = scores
        .iter()
        .find(|s| s.encoder == SeedEncoderId::AstStructure)
    {
        if ast_score.components < 3 {
            recs.push(GenomeRecommendation {
                metric: "components".into(),
                severity: RecommendationSeverity::Warning,
                message: format!(
                    "AST Structure shows {} components. The code is monolithic. \
                     Consider splitting into multiple functions or modules with clear boundaries.",
                    ast_score.components,
                ),
                location: None,
            });
        }
    }

    // Structural encoder recommendations — this encoder is the most common bottleneck.
    // It operates at the raw byte level; detect when it's an outlier and provide
    // specific guidance based on the four byte-level channels.
    analyze_structural_outlier(text, scores, &mut recs);

    // Tree-sitter based recommendations.
    let tree = match ts_parse(text, file_path) {
        Some(t) => t,
        None => return recs,
    };

    let lines: Vec<&str> = text.lines().collect();
    let root = tree.root_node();

    // Walk top-level function nodes for cyclomatic complexity and identifier uniqueness.
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        let kind = child.kind();
        // Detect function-like nodes across languages.
        let is_fn = matches!(
            kind,
            "function_item"
                | "function_definition"
                | "function_declaration"
                | "method_definition"
                | "arrow_function"
                | "impl_item"
        );
        if !is_fn {
            // Also check children (e.g., items inside impl blocks).
            let mut inner = child.walk();
            for grandchild in child.children(&mut inner) {
                if matches!(
                    grandchild.kind(),
                    "function_item" | "function_definition" | "method_definition"
                ) {
                    analyze_function_node(text, &lines, &grandchild, &mut recs);
                }
            }
            continue;
        }
        analyze_function_node(text, &lines, &child, &mut recs);
    }

    // Nesting depth analysis.
    analyze_nesting_depth(text, &root, &mut recs);

    // Token entropy analysis via sliding window.
    analyze_token_entropy(text, &lines, &root, &mut recs);

    recs
}

/// Analyze a single function node for cyclomatic complexity and identifier uniqueness.
fn analyze_function_node(
    text: &str,
    _lines: &[&str],
    node: &tree_sitter::Node<'_>,
    recs: &mut Vec<GenomeRecommendation>,
) {
    let fn_name = find_function_name(text, node).unwrap_or_else(|| "<anonymous>".to_string());
    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;

    // Cyclomatic complexity: count decision points.
    let cc = compute_cyclomatic_complexity(text, node);
    if cc > 10 {
        recs.push(GenomeRecommendation {
            metric: "cyclomatic_complexity".into(),
            severity: RecommendationSeverity::Critical,
            message: format!(
                "Split {fn_name}() (complexity {cc}) into smaller functions. \
                 Consider extracting logic in lines {start_line}-{end_line} into a separate function.",
            ),
            location: Some(format!("{fn_name}:{start_line}-{end_line}")),
        });
    }

    // Identifier uniqueness per function scope.
    let (total_ids, unique_ids) = count_identifiers(text, node);
    if total_ids > 0 {
        let uniqueness = unique_ids as f32 / total_ids as f32;
        if uniqueness < 0.5 {
            let pct = ((1.0 - uniqueness) * 100.0).round() as u32;
            recs.push(GenomeRecommendation {
                metric: "identifier_uniqueness".into(),
                severity: RecommendationSeverity::Warning,
                message: format!(
                    "{pct}% of identifiers in {fn_name} are reused names. \
                     Use descriptive names that reflect purpose.",
                ),
                location: Some(format!("{fn_name}:{start_line}-{end_line}")),
            });
        }
    }
}

/// Extract the function name from a function-like AST node.
fn find_function_name(text: &str, node: &tree_sitter::Node<'_>) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "name"
            || child.kind() == "identifier"
            || child.kind() == "property_identifier"
        {
            return Some(text[child.byte_range()].to_string());
        }
    }
    None
}

/// Compute cyclomatic complexity by counting decision-point nodes in the subtree.
fn compute_cyclomatic_complexity(text: &str, node: &tree_sitter::Node<'_>) -> u32 {
    let mut cc = 1u32; // base complexity
    let mut stack = vec![*node];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "if_expression"
            | "if_statement"
            | "match_expression"
            | "match_statement"
            | "while_expression"
            | "while_statement"
            | "for_expression"
            | "for_statement"
            | "for_in_statement"
            | "loop_expression"
            | "conditional_expression"
            | "ternary_expression" => {
                cc += 1;
            }
            "binary_expression" => {
                let op_text = n
                    .child_by_field_name("operator")
                    .map(|op| &text[op.byte_range()]);
                if matches!(op_text, Some("&&" | "||")) {
                    cc += 1;
                }
            }
            _ => {}
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            // Don't recurse into nested function definitions.
            if matches!(
                child.kind(),
                "function_item"
                    | "function_definition"
                    | "function_declaration"
                    | "method_definition"
                    | "arrow_function"
                    | "closure_expression"
            ) && child.id() != node.id()
            {
                continue;
            }
            stack.push(child);
        }
    }
    cc
}

/// Count total and unique identifiers within a function's subtree.
fn count_identifiers(text: &str, node: &tree_sitter::Node<'_>) -> (usize, usize) {
    let mut all = Vec::new();
    let mut stack = vec![*node];
    while let Some(n) = stack.pop() {
        if n.kind() == "identifier" {
            all.push(text[n.byte_range()].to_string());
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
    let total = all.len();
    let unique: HashSet<_> = all.into_iter().collect();
    (total, unique.len())
}

/// Analyze nesting depth across the file, reporting ranges with depth > 4.
fn analyze_nesting_depth(
    _text: &str,
    root: &tree_sitter::Node<'_>,
    recs: &mut Vec<GenomeRecommendation>,
) {
    let mut max_depth_per_line: HashMap<usize, usize> = HashMap::new();
    collect_depth(root, 0, &mut max_depth_per_line);

    // Find contiguous ranges where depth > 4.
    let mut sorted_lines: Vec<(usize, usize)> = max_depth_per_line
        .iter()
        .filter(|&(_, &d)| d > 4)
        .map(|(&line, &depth)| (line, depth))
        .collect();
    sorted_lines.sort_by_key(|&(line, _)| line);

    if sorted_lines.is_empty() {
        return;
    }

    let mut ranges: Vec<(usize, usize, usize)> = Vec::new(); // (start, end, max_depth)
    let mut start = sorted_lines[0].0;
    let mut end = start;
    let mut max_d = sorted_lines[0].1;
    for &(line, depth) in sorted_lines.iter().skip(1) {
        if line <= end + 2 {
            // contiguous (allow 1-line gap)
            end = line;
            max_d = max_d.max(depth);
        } else {
            ranges.push((start, end, max_d));
            start = line;
            end = line;
            max_d = depth;
        }
    }
    ranges.push((start, end, max_d));

    for (s, e, d) in ranges {
        let s1 = s + 1;
        let e1 = e + 1;
        recs.push(GenomeRecommendation {
            metric: "nesting_depth".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Flatten nesting in lines {s1}-{e1} (depth {d}). \
                 Consider early returns, guard clauses, or extracting the inner block.",
            ),
            location: Some(format!("{s1}-{e1}")),
        });
    }
}

fn collect_depth(node: &tree_sitter::Node<'_>, depth: usize, out: &mut HashMap<usize, usize>) {
    let start_line = node.start_position().row;
    let end_line = node.end_position().row;
    for line in start_line..=end_line {
        let entry = out.entry(line).or_insert(0);
        *entry = (*entry).max(depth);
    }
    let mut cursor = node.walk();
    let child_depth = if is_nesting_node(node.kind()) {
        depth + 1
    } else {
        depth
    };
    for child in node.children(&mut cursor) {
        collect_depth(&child, child_depth, out);
    }
}

fn is_nesting_node(kind: &str) -> bool {
    matches!(
        kind,
        "block"
            | "if_expression"
            | "if_statement"
            | "else_clause"
            | "match_expression"
            | "match_statement"
            | "while_expression"
            | "while_statement"
            | "for_expression"
            | "for_statement"
            | "for_in_statement"
            | "loop_expression"
            | "try_statement"
            | "catch_clause"
    )
}

/// Token entropy analysis: compute Shannon entropy of token types per sliding window of 10 lines.
fn analyze_token_entropy(
    _text: &str,
    lines: &[&str],
    root: &tree_sitter::Node<'_>,
    recs: &mut Vec<GenomeRecommendation>,
) {
    if lines.len() < 10 {
        return;
    }

    // Collect token kinds per line.
    let mut tokens_per_line: Vec<Vec<&str>> = vec![Vec::new(); lines.len()];
    collect_leaf_tokens(root, &mut tokens_per_line);

    // Sliding window of 10 lines.
    let window = 10;
    let mut low_ranges: Vec<(usize, usize, f32)> = Vec::new();

    for start in 0..=lines.len().saturating_sub(window) {
        let end = (start + window).min(lines.len());
        let mut kind_counts: HashMap<&str, usize> = HashMap::new();
        let mut total = 0usize;
        for line_tokens in &tokens_per_line[start..end] {
            for &kind in line_tokens {
                *kind_counts.entry(kind).or_insert(0) += 1;
                total += 1;
            }
        }
        if total < 5 {
            continue;
        }
        let entropy = shannon_entropy(&kind_counts, total);
        if entropy < 3.0 {
            match low_ranges.last_mut() {
                Some((_, ref mut prev_end, ref mut min_e)) if start <= *prev_end + 2 => {
                    *prev_end = end;
                    if entropy < *min_e {
                        *min_e = entropy;
                    }
                }
                _ => {
                    low_ranges.push((start, end, entropy));
                }
            }
        }
    }

    for (s, e, val) in low_ranges {
        let s1 = s + 1;
        let e1 = e;
        recs.push(GenomeRecommendation {
            metric: "token_entropy".into(),
            severity: RecommendationSeverity::Info,
            message: format!(
                "Lines {s1}-{e1} have low token diversity (entropy {val:.1}). \
                 This may indicate copy-paste code. Consider extracting a shared abstraction.",
            ),
            location: Some(format!("{s1}-{e1}")),
        });
    }
}

fn collect_leaf_tokens<'a>(node: &tree_sitter::Node<'a>, out: &mut Vec<Vec<&'a str>>) {
    if node.child_count() == 0 {
        let line = node.start_position().row;
        if line < out.len() {
            out[line].push(node.kind());
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_leaf_tokens(&child, out);
    }
}

fn shannon_entropy(counts: &HashMap<&str, usize>, total: usize) -> f32 {
    if total == 0 {
        return 0.0;
    }
    let t = total as f64;
    let mut entropy = 0.0f64;
    for &count in counts.values() {
        if count == 0 {
            continue;
        }
        let p = count as f64 / t;
        entropy -= p * p.log2();
    }
    entropy as f32
}

// ---------------------------------------------------------------------------
// Structural encoder outlier analysis
// ---------------------------------------------------------------------------

/// Detect when the structural encoder is a significant outlier and generate
/// targeted recommendations based on the four token-role channels:
/// role diversity (35%), AST depth (25%), role entropy (20%), role n-gram (20%).
fn analyze_structural_outlier(
    _text: &str,
    scores: &[EncoderScore],
    recs: &mut Vec<GenomeRecommendation>,
) {
    let structural = match scores
        .iter()
        .find(|s| s.encoder == SeedEncoderId::Structural)
    {
        Some(s) => s,
        None => return,
    };

    // Check if structural is an outlier relative to the other encoders.
    let ast_gens: Vec<u32> = scores
        .iter()
        .filter(|s| s.encoder != SeedEncoderId::Structural)
        .map(|s| s.generations_survived)
        .collect();
    if ast_gens.is_empty() {
        return;
    }
    let ast_mean = ast_gens.iter().sum::<u32>() as f32 / ast_gens.len() as f32;

    // Only diagnose if structural is significantly below the AST encoders.
    let is_outlier = ast_mean > 50.0 && (structural.generations_survived as f32) < ast_mean * 0.3;
    if !is_outlier {
        return;
    }

    // The structural encoder operates on semantic token roles from tree-sitter.
    // Low scores indicate: too few distinct roles per region, flat AST depth,
    // low role entropy, or repeated role n-gram patterns.
    let mut specific = false;

    // Low density suggests few distinct roles or very flat depth.
    if structural.density < 0.10 {
        recs.push(GenomeRecommendation {
            metric: "structural_diversity".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Structural encoder bottleneck: density {:.2} is very low. The token-role \
                 distribution lacks variety. This usually means code is structurally repetitive. \
                 Solve different sub-problems with naturally different approaches rather than \
                 repeating the same pattern. Do NOT add comments just for diversity.",
                structural.density,
            ),
            location: None,
        });
        specific = true;
    }

    // Low components suggests the GoL seed is too uniform — role patterns repeat.
    if structural.components < 5 {
        recs.push(GenomeRecommendation {
            metric: "structural_ngram".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Structural encoder bottleneck: only {} connected regions in the GoL grid. \
                 The code has too many repeated structural patterns. Functions likely share \
                 the same role sequence (e.g., keyword-variable-operator-punctuation). Vary \
                 function signatures, error handling styles, and intersperse different node \
                 types (closures, trait impls, enums, const items).",
                structural.components,
            ),
            location: None,
        });
        specific = true;
    }

    if !specific {
        recs.push(GenomeRecommendation {
            metric: "structural_general".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Structural encoder is a severe outlier ({} generations vs {:.0} AST mean). \
                 This encoder measures token-role diversity, AST depth variation, role entropy, \
                 and role-pattern uniqueness. The code likely has repeated structural patterns. \
                 Write naturally varied code — different sub-problems should produce different \
                 shapes. Do NOT add comments or artificial variety to game this encoder.",
                structural.generations_survived, ast_mean,
            ),
            location: None,
        });
    }
}

#[cfg(test)]
#[path = "tests/genome_report.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/genome_check.rs"]
mod genome_check;
