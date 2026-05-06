//! Centralized tests for the seed encoders — invariants on empty input,
//! tree-sitter fallback, and AST-driven encoder smoke probes.

use super::*;

#[test]
fn seed_encoders_do_not_panic_on_empty_input() {
    let input = SeedInput {
        text: "",
        source: GolSeedSource::Editor,
        file_path: None,
        version: 0,
    };
    let params = SeedParams::default();
    for encoder in [
        SeedEncoderId::AsciiBytes,
        SeedEncoderId::Lifehash16,
        SeedEncoderId::HilbertBits,
        SeedEncoderId::TokenSpectrum,
        SeedEncoderId::AstStructure,
        SeedEncoderId::ComplexityField,
    ] {
        let encoded = encode_seed(&input, encoder, &params, 0, 0, 32, 32);
        assert_eq!(encoded.grid.width(), 32);
        assert_eq!(encoded.grid.height(), 32);
    }
}

#[test]
fn token_spectrum_with_rust_source_produces_nonuniform_grid() {
    let rust_source = r#"
fn main() {
    let x = 42;
    if x > 10 {
        println!("Hello, world!");
    }
}

struct Foo {
    bar: String,
    baz: usize,
}

impl Foo {
    fn new(bar: String) -> Self {
        Self { bar, baz: 0 }
    }
}
"#;
    let path = std::path::Path::new("test.rs");
    let input = SeedInput {
        text: rust_source,
        source: GolSeedSource::Editor,
        file_path: Some(path),
        version: 0,
    };
    let grid = TokenSpectrumEncoder.encode(&input, 0, 0);
    assert_eq!(grid.width(), 32);
    assert_eq!(grid.height(), 32);

    // Verify non-uniform: at least 5 distinct values.
    let mut seen = std::collections::HashSet::new();
    for v in grid.values() {
        seen.insert(*v);
    }
    assert!(
        seen.len() >= 5,
        "Expected diverse values, got {} distinct",
        seen.len()
    );
}

#[test]
fn ast_structure_with_rust_source_produces_nonuniform_grid() {
    let rust_source = r#"
fn compute(x: i32) -> i32 {
    if x > 0 {
        x * 2
    } else {
        x + 1
    }
}

fn helper(a: &str, b: &str) -> String {
    format!("{a}{b}")
}
"#;
    let path = std::path::Path::new("test.rs");
    let input = SeedInput {
        text: rust_source,
        source: GolSeedSource::Editor,
        file_path: Some(path),
        version: 0,
    };
    let grid = AstStructureEncoder.encode(&input, 0, 0);
    assert_eq!(grid.width(), 32);

    let mut seen = std::collections::HashSet::new();
    for v in grid.values() {
        seen.insert(*v);
    }
    assert!(
        seen.len() >= 5,
        "Expected diverse values, got {} distinct",
        seen.len()
    );
}

#[test]
fn complexity_field_with_rust_source_produces_nonuniform_grid() {
    let rust_source = r#"
fn complex_function(x: i32, y: i32) -> i32 {
    if x > 0 {
        if y > 0 {
            x + y
        } else {
            x - y
        }
    } else if x == 0 {
        match y {
            0 => 0,
            1 => 1,
            _ => y * 2,
        }
    } else {
        -x
    }
}

fn simple() -> bool {
    true
}
"#;
    let path = std::path::Path::new("test.rs");
    let input = SeedInput {
        text: rust_source,
        source: GolSeedSource::Editor,
        file_path: Some(path),
        version: 0,
    };
    let grid = ComplexityFieldEncoder.encode(&input, 0, 0);
    assert_eq!(grid.width(), 32);

    let mut seen = std::collections::HashSet::new();
    for v in grid.values() {
        seen.insert(*v);
    }
    assert!(
        seen.len() >= 3,
        "Expected diverse values, got {} distinct",
        seen.len()
    );
}

#[test]
fn new_encoders_with_gibberish_fall_back_gracefully() {
    let gibberish = "asdfjkl;asdfjkl;asdfjkl;asdfjkl;asdfjkl;";
    let input = SeedInput {
        text: gibberish,
        source: GolSeedSource::Editor,
        file_path: None, // no file path → PlainText → no tree-sitter
        version: 0,
    };
    for encoder_id in [
        SeedEncoderId::TokenSpectrum,
        SeedEncoderId::AstStructure,
        SeedEncoderId::ComplexityField,
    ] {
        let params = SeedParams::default();
        let encoded = encode_seed(&input, encoder_id, &params, 0, 0, 32, 32);
        assert_eq!(encoded.grid.width(), 32);
        assert_eq!(encoded.grid.height(), 32);
    }
}

#[test]
fn clean_code_vs_gibberish_produces_different_grids() {
    let clean = r#"
fn fibonacci(n: u64) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        _ => fibonacci(n - 1) + fibonacci(n - 2),
    }
}

fn is_prime(n: u64) -> bool {
    if n <= 1 {
        return false;
    }
    let mut i = 2;
    while i * i <= n {
        if n % i == 0 {
            return false;
        }
        i += 1;
    }
    true
}
"#;
    let gibberish = "x".repeat(clean.len());
    let clean_path = std::path::Path::new("clean.rs");
    let clean_input = SeedInput {
        text: clean,
        source: GolSeedSource::Editor,
        file_path: Some(clean_path),
        version: 0,
    };
    let gib_input = SeedInput {
        text: &gibberish,
        source: GolSeedSource::Editor,
        file_path: Some(clean_path),
        version: 0,
    };

    for encoder in [
        SeedEncoderId::TokenSpectrum,
        SeedEncoderId::AstStructure,
        SeedEncoderId::ComplexityField,
    ] {
        let params = SeedParams::default();
        let clean_encoded = encode_seed(&clean_input, encoder, &params, 0, 0, 32, 32);
        let gib_encoded = encode_seed(&gib_input, encoder, &params, 0, 0, 32, 32);

        // Both should produce non-zero density (visible grids).
        assert!(
            clean_encoded.stats.density > 0.0,
            "Encoder {encoder:?}: clean code should have non-zero density"
        );

        // The grids should differ — different inputs produce different patterns.
        let diff_count = clean_encoded
            .base_values
            .values()
            .iter()
            .zip(gib_encoded.base_values.values().iter())
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            diff_count > 100,
            "Encoder {encoder:?}: clean and gibberish grids should differ substantially, but only {diff_count} cells differ"
        );
    }
}

#[allow(dead_code)]
fn grid_variance(grid: &SeedValueGrid) -> f64 {
    let values = grid.values();
    if values.is_empty() {
        return 0.0;
    }
    let mean = values.iter().map(|&v| v as f64).sum::<f64>() / values.len() as f64;
    values
        .iter()
        .map(|&v| {
            let d = v as f64 - mean;
            d * d
        })
        .sum::<f64>()
        / values.len() as f64
}

#[test]
fn unsupported_language_fallback() {
    let text = "some random content that has no known extension";
    let path = std::path::Path::new("unknown.xyz");
    let input = SeedInput {
        text,
        source: GolSeedSource::Editor,
        file_path: Some(path),
        version: 0,
    };
    for encoder_id in [
        SeedEncoderId::TokenSpectrum,
        SeedEncoderId::AstStructure,
        SeedEncoderId::ComplexityField,
    ] {
        let params = SeedParams::default();
        let encoded = encode_seed(&input, encoder_id, &params, 0, 0, 32, 32);
        assert_eq!(encoded.grid.width(), 32);
        assert_eq!(encoded.grid.height(), 32);
        // Should not be all zeros.
        let has_nonzero = encoded.base_values.values().iter().any(|&v| v > 0);
        assert!(has_nonzero, "Encoder {encoder_id:?} produced all-zero grid");
    }
}

#[test]
fn seed_encoder_id_from_str_roundtrip() {
    for id in [
        SeedEncoderId::AsciiBytes,
        SeedEncoderId::Lifehash16,
        SeedEncoderId::HilbertBits,
        SeedEncoderId::Structural,
        SeedEncoderId::TokenSpectrum,
        SeedEncoderId::AstStructure,
        SeedEncoderId::ComplexityField,
    ] {
        assert_eq!(SeedEncoderId::from_str_name(id.as_str()), Some(id));
    }
    assert_eq!(SeedEncoderId::from_str_name("nonexistent"), None);
}
