//! Genome report engine: structural quality feedback for agent code changes.
//!
//! Runs all seven seed encoders on a source file, simulates Conway's Game of Life
//! on each resulting grid, and produces a [`GenomeReport`] with per-encoder metrics,
//! a composite tier, cross-encoder consistency, and targeted recommendations.

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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EncoderScore {
    pub encoder: SeedEncoderId,
    pub density: f32,
    pub components: usize,
    pub generations_survived: u32,
    pub peak_population: u32,
    pub cycle_period: Option<u32>,
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
            (GenomeTier::Oscillator, c) if c >= 0.30 => "Minimum",
            _ => "Failing",
        }
    }
}

pub const GENOME_AGENT_INSTRUCTIONS: &str = "\
You are writing code in nit, which measures structural quality by encoding your \
source file as a Game of Life genome. Your code is evaluated across four \
actionable encoders. Each captures a different dimension of code quality. \
Cross-encoder consistency measures how much they agree — low consistency means \
some dimensions are strong but others are weak. Focus on the weakest encoder.\n\
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
Hybrid encoder:\n\
  structural — Shannon entropy, bracket depth balance, token signal density, \
n-gram uniqueness.\n\
    → Balance bracket depth (avoid deeply nested blocks). Vary syntax \
patterns (avoid repetitive match arms, if-else chains, or boilerplate). \
Use diverse token patterns across the file.\n\
\n\
TARGETS:\n\
- Tier III (Spaceship, 201+ generations) or higher on all AST encoders.\n\
- Density between 0.20 and 0.35 on AST encoders.\n\
- >= 5 components on ast_structure.\n\
- Cyclomatic complexity <= 8 per function.\n\
- Nesting depth <= 3 on average.\n\
- Identifier uniqueness >= 65% per scope.\n\
- Cross-encoder consistency >= 0.60.\n\
\n\
When you see an OUTLIER encoder in the scores, that encoder is the bottleneck. \
Use the encoder guide above to determine what specific code changes will \
improve it. The fastest path to better quality is raising the weakest encoder, \
not improving one that's already strong.\n\
\n\
After writing code, use the evaluate_genome tool to check your structural score. \
If tier drops below III, refactor before submitting.";

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
const GRID_SIZE: usize = 32;

// ---------------------------------------------------------------------------
// Core computation
// ---------------------------------------------------------------------------

/// All seven encoder IDs in canonical order.
const ALL_ENCODERS: [SeedEncoderId; 7] = [
    SeedEncoderId::AsciiBytes,
    SeedEncoderId::Lifehash16,
    SeedEncoderId::HilbertBits,
    SeedEncoderId::Structural,
    SeedEncoderId::TokenSpectrum,
    SeedEncoderId::AstStructure,
    SeedEncoderId::ComplexityField,
];

/// AST-driven encoder IDs used for tier determination.
const AST_ENCODERS: [SeedEncoderId; 3] = [
    SeedEncoderId::TokenSpectrum,
    SeedEncoderId::AstStructure,
    SeedEncoderId::ComplexityField,
];

/// Actionable encoders: AST-driven + hybrid. These are the encoders whose scores
/// the agent can meaningfully improve through structural code changes.
/// Byte-level encoders (ascii_bytes, hilbert_bits, lifehash16) are excluded because
/// they measure surface-level byte patterns that add noise to quality signals.
const ACTIONABLE_ENCODERS: [SeedEncoderId; 4] = [
    SeedEncoderId::TokenSpectrum,
    SeedEncoderId::AstStructure,
    SeedEncoderId::ComplexityField,
    SeedEncoderId::Structural,
];

/// Compute a full genome report for the given source text and file path.
pub fn compute_genome_report(text: &str, file_path: &Path) -> GenomeReport {
    let input = SeedInput {
        text,
        source: GolSeedSource::Editor,
        file_path: Some(file_path),
        version: 0,
    };
    let params = SeedParams::default();
    let conway = Rule::conway();

    // Step 1-4: encode each encoder via encode_seed(), then simulate GoL.
    let mut encoder_scores = Vec::with_capacity(7);
    for &encoder_id in &ALL_ENCODERS {
        let encoded = encode_seed(&input, encoder_id, &params, 0, 0, GRID_SIZE, GRID_SIZE);
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

    for gen in 1..=MAX_GENERATIONS {
        grid = nit_gol::step::step(&grid, rule, EdgeMode::Dead);
        let alive = grid.alive_count() as u32;
        if alive > peak_population {
            peak_population = alive;
        }
        // All cells dead — no further evolution possible.
        if alive == 0 {
            generations_survived = gen;
            break;
        }
        let h = grid.hash();
        if let Some(&first_seen) = seen.get(&h) {
            generations_survived = gen;
            cycle_period = Some(gen - first_seen);
            break;
        }
        seen.insert(h, gen);
    }
    if cycle_period.is_none() && generations_survived == 0 {
        generations_survived = MAX_GENERATIONS;
    }

    EncoderScore {
        encoder,
        density,
        components,
        generations_survived,
        peak_population,
        cycle_period,
    }
}

/// Compute cross-encoder consistency from actionable encoders only (AST + structural).
/// Byte-level encoders are excluded — they add noise that doesn't reflect
/// code quality the agent can act on.
fn compute_consistency(scores: &[EncoderScore]) -> f32 {
    let gens: Vec<f64> = scores
        .iter()
        .filter(|s| ACTIONABLE_ENCODERS.contains(&s.encoder))
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
        "Tier: {} ({})\n",
        report.tier.numeral(),
        report.tier.name()
    ));
    out.push_str(&format!(
        "Cross-encoder consistency: {:.2}\n\n",
        report.cross_encoder_consistency
    ));

    out.push_str("Encoder scores:\n");
    for score in &report.encoder_scores {
        out.push_str(&format!(
            "  {}: density={:.2}, components={}, generations={}, peak_pop={}{}\n",
            score.encoder.label(),
            score.density,
            score.components,
            score.generations_survived,
            score.peak_population,
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

    // Density and component recommendations (no tree-sitter needed).
    for score in scores {
        if matches!(
            score.encoder,
            SeedEncoderId::TokenSpectrum
                | SeedEncoderId::AstStructure
                | SeedEncoderId::ComplexityField
        ) && score.density > 0.45
        {
            recs.push(GenomeRecommendation {
                metric: "density".into(),
                severity: RecommendationSeverity::Warning,
                message: format!(
                    "{} density is {:.2}. The file has insufficient whitespace and comments. \
                     Add documentation comments to public functions and blank lines between logical sections.",
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

#[cfg(test)]
#[path = "tests/genome_report.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/genome_check.rs"]
mod genome_check;
