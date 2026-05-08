//! Seed encoder invariants: empty / fallback inputs survive without panic,
//! AST-driven encoders produce non-uniform grids on real source, and the
//! encoder-id round-trips through its string form.

use super::*;
use crate::config::GolSeedSource;
use std::path::Path;

const GRID_SIDE: usize = 32;

const RUST_SAMPLE_BRANCHY: &str = r#"
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

const RUST_SAMPLE_SMALL: &str = r#"
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

const RUST_SAMPLE_NESTED: &str = r#"
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

const RUST_SAMPLE_RICH: &str = r#"
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

fn make_input<'a>(text: &'a str, file_path: Option<&'a Path>) -> SeedInput<'a> {
    SeedInput {
        text,
        source: GolSeedSource::Editor,
        file_path,
        version: 0,
    }
}

fn unique_value_count(grid: &SeedValueGrid) -> usize {
    let mut seen = std::collections::HashSet::new();
    for v in grid.data() {
        seen.insert(*v);
    }
    seen.len()
}

const ALL_ENCODERS: [SeedEncoderId; 6] = [
    SeedEncoderId::AsciiBytes,
    SeedEncoderId::Lifehash16,
    SeedEncoderId::HilbertBits,
    SeedEncoderId::TokenSpectrum,
    SeedEncoderId::AstStructure,
    SeedEncoderId::ComplexityField,
];

const AST_DRIVEN_ENCODERS: [SeedEncoderId; 3] = [
    SeedEncoderId::TokenSpectrum,
    SeedEncoderId::AstStructure,
    SeedEncoderId::ComplexityField,
];

fn assert_grid_nonuniform(grid: &SeedValueGrid, min_unique: usize, label: &str) {
    let unique = unique_value_count(grid);
    assert_eq!(grid.width(), GRID_SIDE);
    assert_eq!(grid.height(), GRID_SIDE);
    assert!(
        unique >= min_unique,
        "{label}: expected >= {min_unique} distinct values, got {unique}",
    );
}

#[test]
fn seed_encoders_do_not_panic_on_empty_input() {
    let input = make_input("", None);
    let params = SeedParams::default();
    for encoder in ALL_ENCODERS {
        let encoded = encode_seed(&input, encoder, &params, 0, 0, GRID_SIDE, GRID_SIDE);
        assert_eq!(encoded.grid.width(), GRID_SIDE);
        assert_eq!(encoded.grid.height(), GRID_SIDE);
    }
}

#[test]
fn token_spectrum_with_rust_source_produces_nonuniform_grid() {
    let input = make_input(RUST_SAMPLE_BRANCHY, Some(Path::new("test.rs")));
    let grid = TokenSpectrumEncoder.encode(&input, 0, 0);
    assert_grid_nonuniform(&grid, 5, "TokenSpectrum");
}

#[test]
fn ast_structure_with_rust_source_produces_nonuniform_grid() {
    let input = make_input(RUST_SAMPLE_SMALL, Some(Path::new("test.rs")));
    let grid = AstStructureEncoder.encode(&input, 0, 0);
    assert_grid_nonuniform(&grid, 5, "AstStructure");
}

#[test]
fn complexity_field_with_rust_source_produces_nonuniform_grid() {
    let input = make_input(RUST_SAMPLE_NESTED, Some(Path::new("test.rs")));
    let grid = ComplexityFieldEncoder.encode(&input, 0, 0);
    assert_grid_nonuniform(&grid, 3, "ComplexityField");
}

#[test]
fn ast_encoders_with_gibberish_fall_back_gracefully() {
    // No file path → PlainText path → no tree-sitter parsing.
    let input = make_input("asdfjkl;asdfjkl;asdfjkl;asdfjkl;asdfjkl;", None);
    let params = SeedParams::default();
    for encoder_id in AST_DRIVEN_ENCODERS {
        let encoded = encode_seed(&input, encoder_id, &params, 0, 0, GRID_SIDE, GRID_SIDE);
        assert_eq!(encoded.grid.width(), GRID_SIDE);
        assert_eq!(encoded.grid.height(), GRID_SIDE);
    }
}

#[test]
fn clean_code_vs_gibberish_produces_different_grids() {
    let gibberish = "x".repeat(RUST_SAMPLE_RICH.len());
    let path = Path::new("clean.rs");
    let clean_input = make_input(RUST_SAMPLE_RICH, Some(path));
    let gib_input = make_input(&gibberish, Some(path));

    let params = SeedParams::default();
    for encoder in AST_DRIVEN_ENCODERS {
        let clean_encoded = encode_seed(&clean_input, encoder, &params, 0, 0, GRID_SIDE, GRID_SIDE);
        let gib_encoded = encode_seed(&gib_input, encoder, &params, 0, 0, GRID_SIDE, GRID_SIDE);

        assert!(
            clean_encoded.stats.density > 0.0,
            "Encoder {encoder:?}: clean code should have non-zero density"
        );
        let diff_count = clean_encoded
            .base_values
            .data()
            .iter()
            .zip(gib_encoded.base_values.data().iter())
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            diff_count > 100,
            "Encoder {encoder:?}: clean and gibberish grids differ in only {diff_count} cells"
        );
    }
}

#[test]
fn unsupported_language_fallback_still_produces_output() {
    let input = make_input(
        "some random content that has no known extension",
        Some(Path::new("unknown.xyz")),
    );
    let params = SeedParams::default();
    for encoder_id in AST_DRIVEN_ENCODERS {
        let encoded = encode_seed(&input, encoder_id, &params, 0, 0, GRID_SIDE, GRID_SIDE);
        assert_eq!(encoded.grid.width(), GRID_SIDE);
        assert_eq!(encoded.grid.height(), GRID_SIDE);
        let has_nonzero = encoded.base_values.data().iter().any(|&v| v > 0);
        assert!(has_nonzero, "Encoder {encoder_id:?} produced all-zero grid");
    }
}

#[test]
fn seed_encoder_id_from_str_roundtrip() {
    let ids = [
        SeedEncoderId::AsciiBytes,
        SeedEncoderId::Lifehash16,
        SeedEncoderId::HilbertBits,
        SeedEncoderId::Structural,
        SeedEncoderId::TokenSpectrum,
        SeedEncoderId::AstStructure,
        SeedEncoderId::ComplexityField,
    ];
    for id in ids {
        assert_eq!(SeedEncoderId::from_str_name(id.as_str()), Some(id));
    }
    assert_eq!(SeedEncoderId::from_str_name("nonexistent"), None);
}
