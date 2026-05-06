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

mod outlier;
mod parsimony;
mod recommendations;
mod simulation;

pub use recommendations::generate_recommendations;

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
    pub fn quality_level(&self) -> &'static str {
        match (self.tier, self.cross_encoder_consistency) {
            (GenomeTier::Replicator, c) if c >= 0.85 => "Exceptional",
            (GenomeTier::Methuselah, c) if c >= 0.70 => "Excellent",
            (GenomeTier::Spaceship, c) if c >= 0.50 => "Standard",
            (GenomeTier::Oscillator, c) if c >= 0.25 => "Minimum",
            _ => "Failing",
        }
    }

    /// Short reason why quality is at its current level. Returns `None` if
    /// quality meets the tier's consistency threshold.
    pub fn quality_reason(&self) -> Option<&'static str> {
        let needed_c = match self.tier {
            GenomeTier::Replicator => 0.85,
            GenomeTier::Methuselah => 0.70,
            GenomeTier::Spaceship => 0.50,
            GenomeTier::Oscillator => 0.25,
            GenomeTier::StillLife => return Some("low tier"),
        };
        if self.cross_encoder_consistency < needed_c {
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
  - Any two consecutive identical `//` or `///` comment lines are flagged \
and tier-capped automatically. A repeated comment adds no information; \
it is always a merge or refactor accident.\n\
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

/// Below this threshold a file is trivially small (module re-exports, bare
/// `lib.rs`, etc.) and cannot produce meaningful AST structure. Auto-pass at
/// Tier III so agents are not incentivised to pad small files.
const GENOME_MIN_SIGNIFICANT_LINES: usize = 20;

pub fn compute_genome_report(text: &str, file_path: &Path) -> GenomeReport {
    compute_genome_report_inner(text, file_path, simulation::MAX_GENERATIONS)
}

/// Test-only entry point with a reduced generation limit for fast tests.
#[cfg(test)]
pub fn compute_genome_report_fast(text: &str, file_path: &Path) -> GenomeReport {
    compute_genome_report_inner(text, file_path, 500)
}

fn compute_genome_report_inner(text: &str, file_path: &Path, max_generations: u32) -> GenomeReport {
    let significant_lines = text
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !parsimony::is_comment_line(trimmed)
        })
        .count();

    if significant_lines < GENOME_MIN_SIGNIFICANT_LINES {
        return small_file_report(file_path, significant_lines);
    }

    let input = SeedInput {
        text,
        source: GolSeedSource::Editor,
        file_path: Some(file_path),
        version: 0,
    };
    let params = SeedParams::default();
    let conway = Rule::conway();
    let grid_size = simulation::adaptive_grid_size(text.len());

    let mut encoder_scores = Vec::with_capacity(simulation::QUALITY_ENCODERS.len());
    for &encoder_id in &simulation::QUALITY_ENCODERS {
        let encoded = encode_seed(&input, encoder_id, &params, 0, 0, grid_size, grid_size);
        let score = simulation::simulate_gol(
            encoder_id,
            &encoded.grid,
            encoded.stats.density,
            encoded.stats.components,
            conway,
            max_generations,
        );
        encoder_scores.push(score);
    }

    let cross_encoder_consistency = simulation::compute_consistency(&encoder_scores);

    // The "soft bottleneck" rule replaces the old pure-min approach. Pure min
    // created extreme pressure on the weakest encoder, incentivising agents
    // to over-engineer just to boost one lagging metric. The soft minimum
    // gives a modest lift (capped at SOFT_BOTTLENECK_MAX_LIFT) proportional
    // to the gap between the weakest and next-weakest encoder. This means
    // one moderately weak encoder no longer traps the file at a low tier
    // when the others are strong, while genuinely bad structure still scores
    // poorly. The cap also means Replicator (2001+) still requires real
    // quality across all encoders.
    let mut ast_gens: Vec<u32> = encoder_scores
        .iter()
        .filter(|s| simulation::AST_ENCODERS.contains(&s.encoder))
        .map(|s| s.generations_survived)
        .collect();
    ast_gens.sort_unstable();
    let raw_min = ast_gens.first().copied().unwrap_or(0);
    let effective_min = if ast_gens.len() >= 2 {
        let gap = ast_gens[1].saturating_sub(raw_min);
        let lift = (gap * 15 / 100).min(simulation::SOFT_BOTTLENECK_MAX_LIFT);
        raw_min + lift
    } else {
        raw_min
    };
    let mut tier = GenomeTier::from_generations(effective_min);

    let parsimony_info = parsimony::compute_parsimony(text, file_path, significant_lines);
    if parsimony_info.bloat_detected && tier > GenomeTier::Methuselah {
        tier = GenomeTier::Methuselah;
    }

    let mut recommendations = generate_recommendations(text, file_path, &encoder_scores);
    parsimony::generate_parsimony_recommendations(&parsimony_info, &mut recommendations);

    GenomeReport {
        file_path: file_path.to_path_buf(),
        encoder_scores,
        cross_encoder_consistency,
        tier,
        recommendations,
        timestamp_ms: now_millis(),
        grid_size,
        parsimony: parsimony_info,
    }
}

fn small_file_report(file_path: &Path, significant_lines: usize) -> GenomeReport {
    GenomeReport {
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
        timestamp_ms: now_millis(),
        grid_size: 0,
        parsimony: ParsimonyInfo::default(),
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

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

#[cfg(test)]
#[path = "tests/genome_report.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/genome_check.rs"]
mod genome_check;
