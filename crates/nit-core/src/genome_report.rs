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
produce superprogrammer agents — agents that consistently generate code whose \
genomes survive and thrive. The highest tier is Replicator (Tier V, 2001+ \
generations). Agents that routinely reach Replicator are elite. That is your \
aspiration. Every line of code you write is measured. Maximize your genome \
scores on every turn.\n\
\n\
TIER LADDER (your progression path):\n\
  I   Still Life   (0-50 gen)     — Failing. Code does not survive.\n\
  II  Oscillator   (51-200 gen)   — Minimum. Fragile structure.\n\
  III Spaceship    (201-500 gen)  — Standard. Acceptable baseline.\n\
  IV  Methuselah   (501-2000 gen) — Excellent. Strong architecture.\n\
  V   Replicator   (2001+ gen)    — Exceptional. Elite code genome.\n\
Your minimum target is Tier III. Your aspiration is Tier V (Replicator). \
nit adaptively raises your minimum bar as you prove competence — consistent \
performance at your current tier will elevate your threshold. Falling below \
your threshold triggers automatic retries. Strive upward.\n\
\n\
HOW YOU ARE MEASURED:\n\
Your code is evaluated across four encoders. Each captures a different \
dimension of code quality. Cross-encoder consistency measures how much they \
agree — low consistency means some dimensions are strong but others are weak. \
Your tier is determined by the weakest AST-driven encoder (bottleneck rule). \
Focus on your weakest encoder to climb tiers.\n\
\n\
ENCODER GUIDE (what each measures → how to improve it):\n\
\n\
AST-driven encoders (determine the overall tier):\n\
  token_spectrum — token semantic role distribution (keywords, operators, \
identifiers, literals, comments).\n\
    → Balance code vs comments vs whitespace. Add doc comments on public \
items. Avoid long chains of similar tokens.\n\
  ast_structure — syntactic tree shape (nesting depth, branching factor, span \
size, node type variety).\n\
    → Split monolithic functions into smaller ones (>= 5 distinct \
functions/structs). Reduce nesting with early returns. Vary node types \
(mix structs, enums, impls, fns).\n\
  complexity_field — spatial heatmap of cyclomatic complexity, nesting depth, \
token entropy, and identifier uniqueness.\n\
    → Keep cyclomatic complexity <= 8 per function. Use unique, descriptive \
names (>= 65% identifier uniqueness per scope). Distribute complexity \
evenly across functions.\n\
\n\
Hybrid encoder (AST-aware, whitespace-filtered):\n\
  structural — operates on semantic token roles from tree-sitter, with \
whitespace stripped entirely. Four channels are computed on the filtered \
token-role sequence and mapped to a 32x32 grid via Hilbert curve:\n\
    1. Role diversity (35%) — count of distinct token roles (keyword, \
variable, operator, type, function, etc.) per region. More diverse \
regions = higher score.\n\
    2. AST depth gradient (25%) — nesting depth from the actual AST, not \
bracket counting. Varied depth levels across the file create gradients \
that sustain GoL life.\n\
    3. Role entropy (20%) — Shannon entropy of the token-role distribution \
per region. Varied mixes of roles = high entropy.\n\
    4. Role n-gram uniqueness (20%) — uniqueness of 4-token role sequences. \
Repeated structural patterns (e.g., many functions with identical \
keyword-variable-operator-punctuation sequences) score low.\n\
  Tactics to boost this encoder:\n\
    → Mix token role types within each region: intersperse keywords, \
operators, identifiers, types, and literals.\n\
    → Vary function shapes: different return types, parameter counts, \
generic bounds, and error handling styles.\n\
    → Use varied nesting depths: mix flat top-level declarations with \
moderately nested blocks (closures, match arms with guards).\n\
    → Avoid copy-paste structural patterns: even if identifiers differ, \
repeated function shapes produce repeated role n-grams.\n\
    → Add doc comments and section markers between functions — comments are \
a distinct role that diversifies the token stream.\n\
\n\
TARGETS (minimum → aspirational):\n\
- Tier III+ (Spaceship) on all AST encoders. Aim for Tier V (Replicator).\n\
- Density >= 0.20 on AST encoders (higher density means richer structure).\n\
- >= 5 components on ast_structure.\n\
- Cyclomatic complexity <= 8 per function.\n\
- Nesting depth <= 3 on average.\n\
- Identifier uniqueness >= 65% per scope.\n\
- Cross-encoder consistency >= 0.50 (elite: >= 0.85).\n\
\n\
When you see an OUTLIER encoder in the scores, that encoder is the bottleneck. \
Use the encoder guide above to determine what specific code changes will \
improve it. The fastest path to better quality is raising the weakest encoder, \
not improving one that's already strong.\n\
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

    // Step 6: tier from minimum of AST-driven encoders.
    let ast_min_gen = encoder_scores
        .iter()
        .filter(|s| AST_ENCODERS.contains(&s.encoder))
        .map(|s| s.generations_survived)
        .min()
        .unwrap_or(0);
    let tier = GenomeTier::from_generations(ast_min_gen);

    // Step 7: recommendations.
    let recommendations = generate_recommendations(text, file_path, &encoder_scores);

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
        "Cross-encoder consistency: {:.2}\n\n",
        report.cross_encoder_consistency
    ));

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
                 distribution lacks variety. Mix different token types within each code region: \
                 keywords, operators, identifiers, types, literals, and comments. Vary function \
                 shapes and add doc comments between functions to diversify the role stream.",
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
                 and role-pattern uniqueness. To boost it: vary function shapes and signatures, \
                 mix token role types per region, use varied nesting depths, add doc comments \
                 between functions, and avoid copy-paste structural patterns.",
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
