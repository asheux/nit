use crate::genome_report::*;
use std::path::Path;

const WELL_STRUCTURED_RUST: &str = r#"
/// A simple calculator module.
pub struct Calculator {
    value: f64,
}

impl Calculator {
    /// Create a new calculator with an initial value.
    pub fn new(initial: f64) -> Self {
        Self { value: initial }
    }

    /// Add a value to the accumulator.
    pub fn add(&mut self, x: f64) {
        self.value += x;
    }

    /// Subtract a value from the accumulator.
    pub fn subtract(&mut self, x: f64) {
        self.value -= x;
    }

    /// Multiply the accumulator by a factor.
    pub fn multiply(&mut self, factor: f64) {
        self.value *= factor;
    }

    /// Divide the accumulator. Returns None on zero division.
    pub fn divide(&mut self, divisor: f64) -> Option<f64> {
        if divisor == 0.0 {
            return None;
        }
        self.value /= divisor;
        Some(self.value)
    }

    /// Get the current value.
    pub fn result(&self) -> f64 {
        self.value
    }
}

/// Helper function to format a result.
fn format_result(value: f64, precision: usize) -> String {
    format!("{:.prec$}", value, prec = precision)
}

/// Parse a numeric string.
fn parse_number(input: &str) -> Option<f64> {
    input.trim().parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let mut calc = Calculator::new(10.0);
        calc.add(5.0);
        assert_eq!(calc.result(), 15.0);
        calc.subtract(3.0);
        assert_eq!(calc.result(), 12.0);
    }

    #[test]
    fn test_division() {
        let mut calc = Calculator::new(10.0);
        assert_eq!(calc.divide(2.0), Some(5.0));
        assert_eq!(calc.divide(0.0), None);
    }
}
"#;

const GIBBERISH: &str = "asdfghjklasdfghjklasdfghjklasdfghjklasdfghjklasdfghjkl\
asdfghjklasdfghjklasdfghjklasdfghjklasdfghjklasdfghjkl\
asdfghjklasdfghjklasdfghjklasdfghjklasdfghjklasdfghjkl\
asdfghjklasdfghjklasdfghjklasdfghjklasdfghjklasdfghjkl\
asdfghjklasdfghjklasdfghjklasdfghjklasdfghjklasdfghjkl";

#[test]
fn genome_report_well_structured_rust() {
    let path = Path::new("test.rs");
    let report = compute_genome_report_fast(WELL_STRUCTURED_RUST, path);

    assert_eq!(report.encoder_scores.len(), 4);
    // Well-structured code should score reasonably well on AST-driven encoders.
    for score in &report.encoder_scores {
        if matches!(
            score.encoder,
            crate::seed::SeedEncoderId::TokenSpectrum
                | crate::seed::SeedEncoderId::AstStructure
                | crate::seed::SeedEncoderId::ComplexityField
        ) {
            // Should survive at least some generations.
            assert!(
                score.generations_survived > 0,
                "{} survived 0 generations",
                score.encoder.label()
            );
        }
    }
}

#[test]
fn genome_report_gibberish() {
    let path = Path::new("test.rs");
    let report = compute_genome_report_fast(GIBBERISH, path);

    // Gibberish is trivially small — auto-pass as Spaceship.
    assert!(report.encoder_scores.is_empty());
    assert_eq!(report.tier, GenomeTier::Spaceship);
    assert!(!report.recommendations.is_empty());
}

#[test]
fn genome_report_empty_file() {
    let path = Path::new("empty.rs");
    let report = compute_genome_report_fast("", path);

    // Empty file is trivially small — auto-pass as Spaceship.
    assert!(report.encoder_scores.is_empty());
    assert_eq!(report.tier, GenomeTier::Spaceship);
    // Should not panic.
}

#[test]
fn genome_tier_ordering() {
    assert!(GenomeTier::StillLife < GenomeTier::Oscillator);
    assert!(GenomeTier::Oscillator < GenomeTier::Spaceship);
    assert!(GenomeTier::Spaceship < GenomeTier::Methuselah);
    assert!(GenomeTier::Methuselah < GenomeTier::Replicator);
}

#[test]
fn genome_diff_detects_improvement() {
    let path = Path::new("test.rs");
    let before = GenomeReport {
        file_path: path.to_path_buf(),
        encoder_scores: vec![EncoderScore {
            encoder: crate::seed::SeedEncoderId::TokenSpectrum,
            density: 0.50,
            components: 2,
            generations_survived: 40,
            peak_population: 100,
            cycle_period: Some(2),
            growth_class: GrowthClass::Stable,
        }],
        cross_encoder_consistency: 0.30,
        tier: GenomeTier::StillLife,
        recommendations: Vec::new(),
        timestamp_ms: 0,
        grid_size: 32,
        parsimony: ParsimonyInfo::default(),
    };
    let after = GenomeReport {
        file_path: path.to_path_buf(),
        encoder_scores: vec![EncoderScore {
            encoder: crate::seed::SeedEncoderId::TokenSpectrum,
            density: 0.35,
            components: 5,
            generations_survived: 300,
            peak_population: 200,
            cycle_period: Some(5),
            growth_class: GrowthClass::Stable,
        }],
        cross_encoder_consistency: 0.70,
        tier: GenomeTier::Spaceship,
        recommendations: Vec::new(),
        timestamp_ms: 1,
        grid_size: 32,
        parsimony: ParsimonyInfo::default(),
    };
    let diff = compute_genome_diff(&before, &after);
    assert_eq!(diff.tier_before, GenomeTier::StillLife);
    assert_eq!(diff.tier_after, GenomeTier::Spaceship);
    assert!(diff.tier_after > diff.tier_before);
    assert_eq!(diff.encoder_diffs.len(), 1);
    assert!(diff.encoder_diffs[0].generations_delta > 0);
    assert!(diff.encoder_diffs[0].components_delta > 0);
    assert!(diff.encoder_diffs[0].density_delta < 0.0); // density improved (decreased)
}

#[test]
fn genome_diff_detects_regression() {
    let path = Path::new("test.rs");
    let before = GenomeReport {
        file_path: path.to_path_buf(),
        encoder_scores: vec![EncoderScore {
            encoder: crate::seed::SeedEncoderId::AstStructure,
            density: 0.30,
            components: 6,
            generations_survived: 500,
            peak_population: 250,
            cycle_period: None,
            growth_class: GrowthClass::Stable,
        }],
        cross_encoder_consistency: 0.80,
        tier: GenomeTier::Spaceship,
        recommendations: Vec::new(),
        timestamp_ms: 0,
        grid_size: 32,
        parsimony: ParsimonyInfo::default(),
    };
    let after = GenomeReport {
        file_path: path.to_path_buf(),
        encoder_scores: vec![EncoderScore {
            encoder: crate::seed::SeedEncoderId::AstStructure,
            density: 0.55,
            components: 1,
            generations_survived: 20,
            peak_population: 50,
            cycle_period: Some(2),
            growth_class: GrowthClass::Stable,
        }],
        cross_encoder_consistency: 0.20,
        tier: GenomeTier::StillLife,
        recommendations: Vec::new(),
        timestamp_ms: 1,
        grid_size: 32,
        parsimony: ParsimonyInfo::default(),
    };
    let diff = compute_genome_diff(&before, &after);
    assert!(diff.tier_after < diff.tier_before);
    assert!(diff.encoder_diffs[0].generations_delta < 0);
    assert!(diff.encoder_diffs[0].components_delta < 0);
}

#[test]
fn genome_recommendations_high_density() {
    // A file with very dense code (no whitespace/comments) should trigger density warnings.
    let dense_code = "fn a(){let x=1;let y=2;let z=x+y;println!(\"{}\",z);}\n\
                      fn b(){let x=1;let y=2;let z=x+y;println!(\"{}\",z);}\n\
                      fn c(){let x=1;let y=2;let z=x+y;println!(\"{}\",z);}\n\
                      fn d(){let x=1;let y=2;let z=x+y;println!(\"{}\",z);}\n\
                      fn e(){let x=1;let y=2;let z=x+y;println!(\"{}\",z);}";
    let path = Path::new("dense.rs");
    let report = compute_genome_report_fast(dense_code, path);

    // Check if any AST encoder has high density and a recommendation was generated.
    let has_high_density = report.encoder_scores.iter().any(|s| {
        matches!(
            s.encoder,
            crate::seed::SeedEncoderId::TokenSpectrum
                | crate::seed::SeedEncoderId::AstStructure
                | crate::seed::SeedEncoderId::ComplexityField
        ) && s.density > 0.45
    });

    if has_high_density {
        let has_density_rec = report.recommendations.iter().any(|r| r.metric == "density");
        assert!(
            has_density_rec,
            "Expected density recommendation for high-density code"
        );
    }
}

#[test]
fn genome_recommendations_low_components() {
    // A single monolithic block should have low component count.
    let monolithic = "fn main() { println!(\"hello\"); }";
    let path = Path::new("mono.rs");
    let report = compute_genome_report_fast(monolithic, path);

    if let Some(ast_score) = report
        .encoder_scores
        .iter()
        .find(|s| s.encoder == crate::seed::SeedEncoderId::AstStructure)
    {
        if ast_score.components < 3 {
            let has_component_rec = report
                .recommendations
                .iter()
                .any(|r| r.metric == "components");
            assert!(
                has_component_rec,
                "Expected components recommendation for monolithic code"
            );
        }
    }
}

#[test]
fn genome_report_performance() {
    // Generate a ~10KB Rust file.
    let mut code = String::with_capacity(12_000);
    for i in 0..100 {
        code.push_str(&format!(
            "/// Function number {i}.\n\
             pub fn func_{i}(x: i32) -> i32 {{\n\
             \tlet result = x + {i};\n\
             \tresult\n\
             }}\n\n"
        ));
    }
    assert!(code.len() > 5_000);

    let path = Path::new("perf.rs");
    let start = std::time::Instant::now();
    let _report = compute_genome_report_fast(&code, path);
    let elapsed = start.elapsed();

    // With the fast test limit (500 gens) this should complete well under 2s
    // even in debug builds.
    assert!(
        elapsed.as_millis() < 2000,
        "compute_genome_report_fast took {}ms (limit: 2000ms)",
        elapsed.as_millis()
    );
}

#[test]
fn format_genome_report_includes_all_encoders() {
    let path = Path::new("test.rs");
    let report = compute_genome_report_fast(WELL_STRUCTURED_RUST, path);
    let formatted = format_genome_report(&report);

    let encoder_names = [
        "token_spectrum",
        "ast_structure",
        "complexity_field",
        "structural",
    ];
    for name in &encoder_names {
        assert!(
            formatted.contains(name),
            "Formatted report missing encoder: {name}"
        );
    }
}

#[test]
fn parsimony_detects_over_split_code() {
    // Generate a file with many trivially small functions — classic over-engineering.
    let mut code = String::new();
    for i in 0..25 {
        code.push_str(&format!("fn step_{i}(x: i32) -> i32 {{ x + {i} }}\n\n"));
    }
    // Pad to ensure we cross the significant-lines threshold.
    code.push_str("fn main() {\n");
    for i in 0..25 {
        code.push_str(&format!("    let _ = step_{i}(0);\n"));
    }
    code.push_str("}\n");

    let path = std::path::Path::new("bloated.rs");
    let report = compute_genome_report_fast(&code, path);

    // Should detect bloat: 26 functions averaging ~1-2 lines each.
    assert!(
        report.parsimony.bloat_detected,
        "Expected bloat detection for {} fns averaging {:.1} lines",
        report.parsimony.fn_count, report.parsimony.avg_fn_body_lines,
    );
    // Tier should be capped at Methuselah or below.
    assert!(
        report.tier <= GenomeTier::Methuselah,
        "Expected tier <= Methuselah when bloat detected, got {}",
        report.tier,
    );
    // Should have a parsimony recommendation.
    assert!(
        report
            .recommendations
            .iter()
            .any(|r| r.metric == "parsimony"),
        "Expected parsimony recommendation",
    );
}

#[test]
fn parsimony_does_not_flag_natural_code() {
    // The well-structured calculator is natural code — should NOT be flagged.
    let path = std::path::Path::new("test.rs");
    let report = compute_genome_report_fast(WELL_STRUCTURED_RUST, path);

    assert!(
        !report.parsimony.bloat_detected,
        "Well-structured code should not be flagged as bloated (fn_count={}, avg={:.1})",
        report.parsimony.fn_count, report.parsimony.avg_fn_body_lines,
    );
}

#[test]
fn parsimony_detects_comment_padding() {
    // Generate a file where >40% of non-blank lines are comments.
    // 30 comment lines + 20 code lines = 60% comments.
    let mut code = String::new();
    for i in 0..10 {
        code.push_str(&format!("/// Doc comment line A for func_{i}.\n"));
        code.push_str(&format!("/// Doc comment line B for func_{i}.\n"));
        code.push_str(&format!("/// Doc comment line C for func_{i}.\n"));
        code.push_str(&format!(
            "fn func_{i}(x: i32) -> i32 {{\n    x + {i}\n}}\n\n"
        ));
    }

    let path = std::path::Path::new("padded.rs");
    let report = compute_genome_report_fast(&code, path);

    assert!(
        report.parsimony.comment_ratio > 0.40,
        "Expected comment ratio > 0.40, got {:.2}",
        report.parsimony.comment_ratio,
    );
    assert!(
        report.parsimony.bloat_detected,
        "Expected bloat detection for comment-padded file (ratio={:.2})",
        report.parsimony.comment_ratio,
    );
    assert!(
        report.tier <= GenomeTier::Methuselah,
        "Expected tier <= Methuselah when comment padding detected, got {}",
        report.tier,
    );
    assert!(
        report
            .recommendations
            .iter()
            .any(|r| r.metric == "comment_padding"),
        "Expected comment_padding recommendation",
    );
}

#[test]
fn parsimony_detects_duplicate_doc_comments() {
    // Two consecutive identical `///` lines — the exact pattern that slipped
    // through a refactor in nit-utils/src/hashing.rs. File padded to clear
    // the trivial-file auto-pass threshold.
    let code = "\
/// BLAKE3 digest truncated to 64 bits (little-endian).
/// BLAKE3 digest truncated to 64 bits (little-endian).
#[must_use]
pub fn stable_hash_bytes(data: &[u8]) -> u64 {
    let digest = blake3::hash(data);
    let bytes: [u8; 8] = digest.as_bytes()[..8]
        .try_into()
        .expect(\"blake3 digest is 32 bytes\");
    u64::from_le_bytes(bytes)
}

pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}
";
    let report = compute_genome_report_fast(code, std::path::Path::new("dup.rs"));

    assert!(
        report.parsimony.duplicate_comment_lines >= 1,
        "Expected duplicate comment detection, got {}",
        report.parsimony.duplicate_comment_lines,
    );
    assert!(
        report.parsimony.bloat_detected,
        "Expected bloat flagged for duplicate doc comments",
    );
    assert!(
        report.tier <= GenomeTier::Methuselah,
        "Expected tier capped at Methuselah, got {}",
        report.tier,
    );
    assert!(
        report
            .recommendations
            .iter()
            .any(|r| r.metric == "duplicate_comments"),
        "Expected duplicate_comments recommendation",
    );
}

#[test]
fn parsimony_allows_blank_doc_dividers() {
    // Blank `///` lines used as section separators are fine — only non-blank
    // repeats count. Padded past the trivial-file threshold.
    let code = "\
/// Module overview.
///
/// Another paragraph of the doc.
///
/// Yet another paragraph.
pub fn example_one() -> u32 { 1 }

pub fn example_two() -> u32 { 2 }

pub struct Holder { a: u32, b: u32 }

impl Holder {
    pub fn new(a: u32, b: u32) -> Self { Self { a, b } }
    pub fn sum(&self) -> u32 { self.a + self.b }
    pub fn product(&self) -> u32 { self.a * self.b }
    pub fn diff(&self) -> u32 { self.a.saturating_sub(self.b) }
}
";
    let report = compute_genome_report_fast(code, std::path::Path::new("divider.rs"));

    assert_eq!(
        report.parsimony.duplicate_comment_lines, 0,
        "Blank `///` dividers should not count as duplicates",
    );
}

#[test]
fn parsimony_ignores_non_consecutive_repeats() {
    // Two `// TODO` lines separated by code — not consecutive, should not flag.
    // Padded past the trivial-file threshold so parsimony actually runs.
    let code = "\
// TODO: fix
fn a() -> u32 { 1 }

// TODO: fix
fn b() -> u32 { 2 }

pub struct Pair { x: u32, y: u32 }

impl Pair {
    pub fn new(x: u32, y: u32) -> Self { Self { x, y } }
    pub fn sum(&self) -> u32 { self.x + self.y }
    pub fn product(&self) -> u32 { self.x * self.y }
    pub fn max(&self) -> u32 { self.x.max(self.y) }
    pub fn min(&self) -> u32 { self.x.min(self.y) }
}
";
    let report = compute_genome_report_fast(code, std::path::Path::new("todos.rs"));

    assert_eq!(
        report.parsimony.duplicate_comment_lines, 0,
        "Non-consecutive identical comments should not count",
    );
}

#[test]
fn soft_bottleneck_gives_modest_lift() {
    // With pure min, tier = from_generations(480) = Spaceship.
    // Soft bottleneck should give a small lift from the gap to next encoder.
    use crate::seed::SeedEncoderId;

    let scores = [
        EncoderScore {
            encoder: SeedEncoderId::TokenSpectrum,
            density: 0.3,
            components: 5,
            generations_survived: 1800,
            peak_population: 200,
            cycle_period: None,
            growth_class: GrowthClass::Stable,
        },
        EncoderScore {
            encoder: SeedEncoderId::AstStructure,
            density: 0.3,
            components: 5,
            generations_survived: 480,
            peak_population: 200,
            cycle_period: None,
            growth_class: GrowthClass::Stable,
        },
        EncoderScore {
            encoder: SeedEncoderId::ComplexityField,
            density: 0.3,
            components: 5,
            generations_survived: 1500,
            peak_population: 200,
            cycle_period: None,
            growth_class: GrowthClass::Stable,
        },
    ];

    // Compute effective min manually to verify the formula.
    let mut gens: Vec<u32> = scores.iter().map(|s| s.generations_survived).collect();
    gens.sort_unstable();
    let raw_min = gens[0]; // 480
    let next = gens[1]; // 1500
    let gap = next - raw_min; // 1020
    let lift = (gap * 15 / 100).min(200); // min(153, 200) = 153
    let effective = raw_min + lift; // 633

    assert_eq!(raw_min, 480);
    assert!(lift > 0 && lift <= 200, "lift={lift} should be in (0, 200]");
    // Pure min gives Spaceship (480). Soft min should give Methuselah (633).
    assert_eq!(GenomeTier::from_generations(raw_min), GenomeTier::Spaceship);
    assert_eq!(
        GenomeTier::from_generations(effective),
        GenomeTier::Methuselah,
        "Soft bottleneck should lift from Spaceship to Methuselah (effective={effective})"
    );
}

// ---------------------------------------------------------------------------
// Intensive parsimony tests
// ---------------------------------------------------------------------------

/// Helper: generate N one-liner functions + a main that calls them.
/// Returns code with `n` functions averaging ~1-2 significant lines each.
fn gen_tiny_fns(n: usize) -> String {
    let mut code = String::new();
    for i in 0..n {
        code.push_str(&format!("fn f_{i}(x: i32) -> i32 {{ x + {i} }}\n"));
    }
    code.push_str("fn main() {\n");
    for i in 0..n {
        code.push_str(&format!("    let _ = f_{i}(0);\n"));
    }
    code.push_str("}\n");
    code
}

/// Helper: generate N functions with `body_lines` significant lines each.
fn gen_fns_with_body(n: usize, body_lines: usize) -> String {
    let mut code = String::new();
    for i in 0..n {
        code.push_str(&format!("fn func_{i}(x: i32) -> i32 {{\n"));
        for j in 0..body_lines {
            code.push_str(&format!("    let v{j} = x + {j} + {i};\n"));
        }
        code.push_str(&format!("    v0 + {i}\n}}\n\n"));
    }
    code
}

/// Helper: generate code with a specific comment ratio.
/// `code_fns` functions of ~3 lines, plus `comment_lines` comment lines.
fn gen_with_comments(code_fns: usize, comment_lines_per_fn: usize) -> String {
    let mut code = String::new();
    for i in 0..code_fns {
        for _ in 0..comment_lines_per_fn {
            code.push_str(&format!("/// Documentation for function {i}.\n"));
        }
        code.push_str(&format!(
            "fn func_{i}(x: i32) -> i32 {{\n    let r = x + {i};\n    r\n}}\n\n"
        ));
    }
    code
}

// --- Over-split signal tests ---

#[test]
fn parsimony_over_split_at_boundary_14_fns_no_flag() {
    // 14 fns with 6-line bodies: over-split requires fn_count >= 15 → NO.
    // Bodies > 5 lines → not tiny → tiny-fn fraction is 0% → NO.
    // Neither signal fires → no bloat.
    let code = gen_fns_with_body(14, 6);
    let path = std::path::Path::new("boundary14.rs");
    let report = compute_genome_report_fast(&code, path);

    assert!(
        !report.parsimony.bloat_detected,
        "14 fns with 6-line bodies should not trigger any signal \
         (fn_count={}, avg={:.1}, tiny={:.0}%)",
        report.parsimony.fn_count,
        report.parsimony.avg_fn_body_lines,
        report.parsimony.tiny_fn_fraction * 100.0,
    );
}

#[test]
fn parsimony_over_split_at_boundary_15_fns_flags() {
    // 15 fns = PARSIMONY_MIN_FN_COUNT — should trigger if avg < 3.
    let code = gen_tiny_fns(15);
    let path = std::path::Path::new("boundary15.rs");
    let report = compute_genome_report_fast(&code, path);

    // 15 one-liner fns + 1 main with 15 calls = 16 fns total.
    // One-liners average ~1 significant line. Should trigger.
    assert!(
        report.parsimony.bloat_detected,
        "15+ tiny fns should trigger (fn_count={}, avg={:.1})",
        report.parsimony.fn_count, report.parsimony.avg_fn_body_lines,
    );
}

#[test]
fn parsimony_over_split_does_not_flag_medium_bodies() {
    // 20 fns but with 8-line bodies — avg well above 3.0.
    let code = gen_fns_with_body(20, 8);
    let path = std::path::Path::new("medium_body.rs");
    let report = compute_genome_report_fast(&code, path);

    // avg_fn_body should be > 3.0 so over-split should NOT fire.
    // (tiny-fn might still not fire if bodies are > 5 lines)
    assert!(
        report.parsimony.avg_fn_body_lines > 3.0,
        "Expected avg > 3.0, got {:.1}",
        report.parsimony.avg_fn_body_lines,
    );
    // Check the over-split signal specifically: it requires avg < 3.0.
    let over_split = report.parsimony.fn_count >= 15 && report.parsimony.avg_fn_body_lines < 3.0;
    assert!(
        !over_split,
        "Medium-body functions should not trigger over-split"
    );
}

#[test]
fn parsimony_below_min_lines_no_flag() {
    // A tiny file with many tiny fns but < 40 significant lines.
    // Should not trigger because the file is too small.
    let mut code = String::new();
    for i in 0..8 {
        code.push_str(&format!("fn f_{i}() -> i32 {{ {i} }}\n"));
    }
    let path = std::path::Path::new("small.rs");
    let report = compute_genome_report_fast(&code, path);

    assert!(
        !report.parsimony.bloat_detected,
        "File below min-lines should not be flagged",
    );
}

// --- Comment padding signal tests ---

#[test]
fn parsimony_comment_padding_at_39_percent_no_flag() {
    // Just below 40% — should NOT trigger.
    // 8 fns × 2 comment lines + 3 code lines = 16 comment + 24 code = 40 non-blank.
    // Ratio = 16/40 = 40% — exactly at threshold. Need to be just below.
    // Use 8 fns × 1 comment + 4 code lines = 8 comment + 32 code = 40 total.
    // Ratio = 8/40 = 20% — well below.
    // Better: 10 fns × 3 comments + 3 code lines = 30 comment + 30 code = 60 total.
    // Ratio = 30/60 = 50% — above. Need to dial it.
    // Use gen_with_comments(10, 2): 20 comments + 30 code = 50 total, ratio = 40%.
    // That's at the boundary. Use 1 comment per fn for below:
    let code = gen_with_comments(10, 1);
    let path = std::path::Path::new("low_comments.rs");
    let report = compute_genome_report_fast(&code, path);

    // 10 fns × (1 comment + 3 code) = 10 comment + 30 code = 40 non-blank.
    // ratio = 10/40 = 25% — well below 40%.
    assert!(
        report.parsimony.comment_ratio <= 0.40,
        "Expected comment ratio <= 0.40, got {:.2}",
        report.parsimony.comment_ratio,
    );
    let comment_bloat = report.parsimony.comment_ratio > 0.40;
    assert!(!comment_bloat, "25% comments should not trigger padding");
}

#[test]
fn parsimony_comment_padding_at_50_percent_flags() {
    // Well above 40% — should trigger.
    let code = gen_with_comments(10, 3);
    let path = std::path::Path::new("heavy_comments.rs");
    let report = compute_genome_report_fast(&code, path);

    // 10 fns × (3 comments + 3 code) = 30 comments + 30 code = 60 non-blank.
    // ratio = 30/60 = 50%.
    assert!(
        report.parsimony.comment_ratio > 0.40,
        "Expected comment ratio > 0.40, got {:.2}",
        report.parsimony.comment_ratio,
    );
    assert!(
        report.parsimony.bloat_detected,
        "50% comments should trigger bloat",
    );
    assert!(
        report
            .recommendations
            .iter()
            .any(|r| r.metric == "comment_padding"),
        "Should have comment_padding recommendation",
    );
}

#[test]
fn parsimony_comment_padding_small_file_no_flag() {
    // High comment ratio but file too small (< 40 non-blank lines).
    let mut code = String::new();
    for i in 0..3 {
        code.push_str(&format!(
            "/// Doc A.\n/// Doc B.\n/// Doc C.\nfn f_{i}() {{ }}\n"
        ));
    }
    let path = std::path::Path::new("small_comments.rs");
    let report = compute_genome_report_fast(&code, path);

    // 3 fns × (3 comments + 1 code) = 9 comments + 3 code = 12 non-blank.
    // Even though ratio is 75%, file is too small.
    assert!(
        !report.parsimony.bloat_detected,
        "Small file with high comment ratio should not trigger bloat",
    );
}

// --- Tiny-function fraction tests ---

#[test]
fn parsimony_tiny_fn_11_fns_no_flag() {
    // gen_tiny_fns(n) produces n+1 fns. 10 + 1 = 11 < PARSIMONY_TINY_FN_MIN_COUNT (12).
    let code = gen_tiny_fns(10);
    let path = std::path::Path::new("tiny11.rs");
    let report = compute_genome_report_fast(&code, path);

    let too_many_tiny = report.parsimony.fn_count >= 12 && report.parsimony.tiny_fn_fraction > 0.50;
    assert!(
        !too_many_tiny,
        "11 fns should not trigger tiny-fn check (fn_count={})",
        report.parsimony.fn_count,
    );
}

#[test]
fn parsimony_tiny_fn_13_tiny_fns_flags() {
    // 13 one-liner fns — all tiny, fraction = ~100%.
    let code = gen_tiny_fns(13);
    let path = std::path::Path::new("tiny13.rs");
    let report = compute_genome_report_fast(&code, path);

    assert!(
        report.parsimony.tiny_fn_fraction > 0.50,
        "Expected tiny fraction > 0.50, got {:.2}",
        report.parsimony.tiny_fn_fraction,
    );
    assert!(
        report.parsimony.bloat_detected,
        "13+ tiny fns should trigger bloat (fn_count={}, tiny={:.0}%)",
        report.parsimony.fn_count,
        report.parsimony.tiny_fn_fraction * 100.0,
    );
}

#[test]
fn parsimony_tiny_fn_mixed_no_flag_when_below_50_percent() {
    // 15 fns: 7 tiny (2 lines) + 8 medium (10 lines).
    // Tiny fraction = 7/15 = 46.7% — just below 50%.
    let mut code = String::new();
    for i in 0..7 {
        code.push_str(&format!("fn tiny_{i}(x: i32) -> i32 {{ x + {i} }}\n"));
    }
    for i in 0..8 {
        code.push_str(&format!("fn med_{i}(x: i32) -> i32 {{\n"));
        for j in 0..10 {
            code.push_str(&format!("    let v{j} = x + {j};\n"));
        }
        code.push_str(&format!("    v0 + {i}\n}}\n\n"));
    }
    let path = std::path::Path::new("mixed.rs");
    let report = compute_genome_report_fast(&code, path);

    // 7 tiny out of 15 = 46.7%.
    assert!(
        report.parsimony.tiny_fn_fraction < 0.50 || report.parsimony.fn_count < 12,
        "Expected tiny fraction < 50% or fn_count < 12, got {:.2} with {} fns",
        report.parsimony.tiny_fn_fraction,
        report.parsimony.fn_count,
    );
}

#[test]
fn parsimony_tiny_fn_mixed_flags_when_above_50_percent() {
    // 16 fns: 10 tiny (1 line) + 6 medium (10 lines).
    // Tiny fraction = 10/16 = 62.5% — above 50%.
    let mut code = String::new();
    for i in 0..10 {
        code.push_str(&format!("fn tiny_{i}(x: i32) -> i32 {{ x + {i} }}\n"));
    }
    for i in 0..6 {
        code.push_str(&format!("fn med_{i}(x: i32) -> i32 {{\n"));
        for j in 0..10 {
            code.push_str(&format!("    let v{j} = x + {j};\n"));
        }
        code.push_str(&format!("    v0 + {i}\n}}\n\n"));
    }
    let path = std::path::Path::new("mixed_heavy.rs");
    let report = compute_genome_report_fast(&code, path);

    assert!(
        report.parsimony.fn_count >= 12,
        "Expected >= 12 fns, got {}",
        report.parsimony.fn_count,
    );
    assert!(
        report.parsimony.tiny_fn_fraction > 0.50,
        "Expected tiny fraction > 0.50, got {:.2}",
        report.parsimony.tiny_fn_fraction,
    );
    assert!(
        report.parsimony.bloat_detected,
        "62.5% tiny fns in 16-fn file should trigger bloat",
    );
    assert!(
        report
            .recommendations
            .iter()
            .any(|r| r.metric == "tiny_functions"),
        "Should have tiny_functions recommendation",
    );
}

// --- Tier capping tests ---

#[test]
fn parsimony_bloat_caps_tier_at_methuselah() {
    // Generate over-split code. Even if GoL would give Replicator,
    // tier should be capped at Methuselah.
    let code = gen_tiny_fns(25);
    let path = std::path::Path::new("capped.rs");
    let report = compute_genome_report_fast(&code, path);

    assert!(report.parsimony.bloat_detected);
    assert!(
        report.tier <= GenomeTier::Methuselah,
        "Bloat-detected file should be capped at Methuselah, got {}",
        report.tier,
    );
}

// --- Interaction tests ---

#[test]
fn parsimony_comment_and_split_both_flag() {
    // Both over-split AND comment-padded: each should produce a recommendation.
    let mut code = String::new();
    for i in 0..20 {
        code.push_str(&format!(
            "/// Doc for f_{i}.\n/// More docs.\n/// Even more.\n\
             fn f_{i}(x: i32) -> i32 {{ x + {i} }}\n\n"
        ));
    }
    let path = std::path::Path::new("double_bloat.rs");
    let report = compute_genome_report_fast(&code, path);

    assert!(
        report.parsimony.bloat_detected,
        "Both signals should trigger bloat",
    );
    let has_split_rec = report
        .recommendations
        .iter()
        .any(|r| r.metric == "parsimony" || r.metric == "tiny_functions");
    let has_comment_rec = report
        .recommendations
        .iter()
        .any(|r| r.metric == "comment_padding");
    assert!(
        has_split_rec,
        "Should have over-split or tiny_functions rec"
    );
    assert!(has_comment_rec, "Should have comment_padding rec");
}

#[test]
fn parsimony_non_rust_file_returns_default() {
    // Tree-sitter can't parse .txt — parsimony should return defaults.
    let code = "hello world\n".repeat(50);
    let path = std::path::Path::new("file.txt");
    let report = compute_genome_report_fast(&code, path);

    assert_eq!(report.parsimony.fn_count, 0);
    assert!(!report.parsimony.bloat_detected);
}

#[test]
fn parsimony_real_file_agents_claude() {
    // Run parsimony on the actual agents/claude.rs — audited as over-engineered.
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("nit/src/agents/claude.rs");
    if !base.exists() {
        return; // Skip if file doesn't exist in CI.
    }
    let text = std::fs::read_to_string(&base).unwrap();
    let report = compute_genome_report_fast(&text, &base);

    // Verify parsimony was computed (tree-sitter parsed successfully).
    assert!(
        report.parsimony.fn_count >= 10,
        "Expected many functions in claude.rs, got {}",
        report.parsimony.fn_count,
    );
    // Verify metrics are populated — exact values depend on current file state.
    assert!(report.parsimony.avg_fn_body_lines > 0.0);
    assert!(report.parsimony.tiny_fn_fraction >= 0.0);
    assert!(report.parsimony.comment_ratio >= 0.0);
    eprintln!(
        "claude.rs parsimony: fn_count={}, avg={:.1}, tiny={:.0}%, comments={:.0}%, bloat={}",
        report.parsimony.fn_count,
        report.parsimony.avg_fn_body_lines,
        report.parsimony.tiny_fn_fraction * 100.0,
        report.parsimony.comment_ratio * 100.0,
        report.parsimony.bloat_detected,
    );
}

#[test]
fn parsimony_does_not_treat_deref_as_comment() {
    // Regression: `starts_with('*')` used to include `*ptr = 5` and `*mut_ref`
    // as "comment" lines, inflating comment_ratio AND undercounting real
    // code.  The fix narrows the `*` heuristic to block-comment continuation
    // patterns: bare `*`, `* text`, or `*/`.
    let code = r#"
fn deref_and_mut(ptr: *mut i32) {
    *ptr = 5;
    *ptr += 1;
    *ptr *= 2;
    *ptr = *ptr + *ptr;
    *ptr = (*ptr).saturating_add(1);
    *ptr = 2;
    *ptr = 3;
    *ptr = 4;
    *ptr = 5;
    *ptr = 6;
    *ptr = 7;
    *ptr = 8;
    *ptr = 9;
    *ptr = 10;
    *ptr = 11;
    *ptr = 12;
    *ptr = 13;
}
"#;
    let path = std::path::Path::new("deref.rs");
    let report = compute_genome_report_fast(code, path);
    assert_eq!(
        report.parsimony.comment_ratio, 0.0,
        "Deref lines starting with '*' must not be counted as comments",
    );
}

#[test]
fn parsimony_probe_nit_syntax_captures_rs() {
    // Probe: user reports nit-syntax/captures.rs looks over-engineered but
    // parsimony didn't flag it. Dump metrics so we can see the numbers.
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("nit-syntax/src/captures.rs");
    if !base.exists() {
        return;
    }
    let text = std::fs::read_to_string(&base).unwrap();
    let report = compute_genome_report_fast(&text, &base);
    eprintln!(
        "captures.rs parsimony: fn_count={}, avg={:.2}, tiny={:.2}%, comments={:.2}%, bloat={}, tier={:?}",
        report.parsimony.fn_count,
        report.parsimony.avg_fn_body_lines,
        report.parsimony.tiny_fn_fraction * 100.0,
        report.parsimony.comment_ratio * 100.0,
        report.parsimony.bloat_detected,
        report.tier,
    );
}

#[test]
fn parsimony_format_includes_bloat_tag() {
    // Verify that format_genome_report includes the BLOAT tag when detected.
    let code = gen_tiny_fns(25);
    let path = std::path::Path::new("fmt_bloat.rs");
    let report = compute_genome_report_fast(&code, path);

    assert!(report.parsimony.bloat_detected);
    let formatted = format_genome_report(&report);
    assert!(
        formatted.contains("[BLOAT"),
        "Formatted report should contain [BLOAT tag:\n{formatted}",
    );
    assert!(
        formatted.contains("tiny"),
        "Formatted report should show tiny % in parsimony line:\n{formatted}",
    );
}
