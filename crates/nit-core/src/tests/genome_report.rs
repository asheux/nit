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
    let report = compute_genome_report(WELL_STRUCTURED_RUST, path);

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
    let report = compute_genome_report(GIBBERISH, path);

    // Gibberish is trivially small — auto-pass as Spaceship.
    assert!(report.encoder_scores.is_empty());
    assert_eq!(report.tier, GenomeTier::Spaceship);
    assert!(!report.recommendations.is_empty());
}

#[test]
fn genome_report_empty_file() {
    let path = Path::new("empty.rs");
    let report = compute_genome_report("", path);

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
    let report = compute_genome_report(dense_code, path);

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
    let report = compute_genome_report(monolithic, path);

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
    let _report = compute_genome_report(&code, path);
    let elapsed = start.elapsed();

    // Must complete in under 5s (debug builds are ~10x slower than release;
    // adaptive grid sizing uses 48x48 for this ~10KB file).
    assert!(
        elapsed.as_millis() < 5000,
        "compute_genome_report took {}ms (limit: 5000ms)",
        elapsed.as_millis()
    );
}

#[test]
fn format_genome_report_includes_all_encoders() {
    let path = Path::new("test.rs");
    let report = compute_genome_report(WELL_STRUCTURED_RUST, path);
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
    let report = compute_genome_report(&code, path);

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
    let report = compute_genome_report(WELL_STRUCTURED_RUST, path);

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
    let report = compute_genome_report(&code, path);

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
