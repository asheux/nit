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

    assert_eq!(report.encoder_scores.len(), 7);
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

    assert_eq!(report.encoder_scores.len(), 7);
    // Gibberish is not parseable by tree-sitter, so AST encoders fall back
    // to byte-level analysis.
    for score in &report.encoder_scores {
        assert!(score.generations_survived <= 3000);
    }
}

#[test]
fn genome_report_empty_file() {
    let path = Path::new("empty.rs");
    let report = compute_genome_report("", path);

    assert_eq!(report.encoder_scores.len(), 7);
    assert_eq!(report.tier, GenomeTier::StillLife);
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
        }],
        cross_encoder_consistency: 0.30,
        tier: GenomeTier::StillLife,
        recommendations: Vec::new(),
        timestamp_ms: 0,
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
        }],
        cross_encoder_consistency: 0.70,
        tier: GenomeTier::Spaceship,
        recommendations: Vec::new(),
        timestamp_ms: 1,
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
        }],
        cross_encoder_consistency: 0.80,
        tier: GenomeTier::Spaceship,
        recommendations: Vec::new(),
        timestamp_ms: 0,
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
        }],
        cross_encoder_consistency: 0.20,
        tier: GenomeTier::StillLife,
        recommendations: Vec::new(),
        timestamp_ms: 1,
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

    // Must complete in under 500ms.
    assert!(
        elapsed.as_millis() < 500,
        "compute_genome_report took {}ms (limit: 500ms)",
        elapsed.as_millis()
    );
}

#[test]
fn format_genome_report_includes_all_encoders() {
    let path = Path::new("test.rs");
    let report = compute_genome_report(WELL_STRUCTURED_RUST, path);
    let formatted = format_genome_report(&report);

    let encoder_names = [
        "ascii_bytes",
        "lifehash16",
        "hilbert_bits",
        "structural",
        "token_spectrum",
        "ast_structure",
        "complexity_field",
    ];
    for name in &encoder_names {
        assert!(
            formatted.contains(name),
            "Formatted report missing encoder: {name}"
        );
    }
}
