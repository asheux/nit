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

/// Behaviour-equivalent rewrites the agents can use to game the genome
/// score: comments, whitespace, identifier renames. The class-level fix
/// (encode AST features only, no source-text bytes) makes all of these
/// produce byte-identical encoder grids. These tests are the invariant —
/// they fail today and must pass once the AST-only refactor lands.
mod invariance {
    use super::*;

    fn path() -> Option<&'static Path> {
        Some(Path::new("test.rs"))
    }

    const WITH_COMMENTS: &str = r#"
// Top-level doc summary explaining what this module does
fn compute(x: i32) -> i32 {
    // Step 1: square the input
    let squared = x * x;
    /* Step 2: add a constant offset before returning.
       Multi-line block comment for emphasis. */
    let result = squared + 42;
    result
}

// Helper used by callers in the integration tests
fn helper(a: &str) -> usize {
    a.len() // inline trailing comment
}
"#;

    const NO_COMMENTS: &str = r#"
fn compute(x: i32) -> i32 {
    let squared = x * x;
    let result = squared + 42;
    result
}

fn helper(a: &str) -> usize {
    a.len()
}
"#;

    const NAMES_A: &str = r#"
fn compute(x: i32) -> i32 {
    let squared = x * x;
    let result = squared + 42;
    result
}

fn helper(a: &str) -> usize {
    a.len()
}
"#;

    const NAMES_B: &str = r#"
fn alpha(q: i32) -> i32 {
    let beta = q * q;
    let gamma = beta + 42;
    gamma
}

fn delta(epsilon: &str) -> usize {
    epsilon.len()
}
"#;

    const NORMAL_WHITESPACE: &str = r#"
fn compute(x: i32) -> i32 {
    let squared = x * x;
    let result = squared + 42;
    result
}

fn helper(a: &str) -> usize {
    a.len()
}
"#;

    // Same AST, different whitespace. Extra blank lines, indentation
    // shifted from 4 spaces to 2 spaces, padding spaces around operators.
    const SHIFTED_WHITESPACE: &str = "
fn compute( x : i32 ) -> i32 {

  let squared = x * x ;


  let result = squared + 42 ;
  result
}


fn helper( a : &str ) -> usize {
  a . len()
}
";

    fn grid_diff_count(a: &SeedValueGrid, b: &SeedValueGrid) -> usize {
        assert_eq!(a.width(), b.width());
        assert_eq!(a.height(), b.height());
        a.data()
            .iter()
            .zip(b.data().iter())
            .filter(|(x, y)| x != y)
            .count()
    }

    fn encode_all(text_a: &str, text_b: &str) -> Vec<(SeedValueGrid, SeedValueGrid, &'static str)> {
        // Drive the *full* `encode_seed` pipeline — same path as
        // `compute_genome_report` and the running TUI. The earlier
        // version of these tests called each encoder's `.encode()`
        // directly and bypassed `apply_jitter`, which silently let a
        // byte-hash leak survive in production while passing here.
        let in_a = make_input(text_a, path());
        let in_b = make_input(text_b, path());
        let params = SeedParams::default();
        let ids = [
            (SeedEncoderId::TokenSpectrum, "TokenSpectrum"),
            (SeedEncoderId::AstStructure, "AstStructure"),
            (SeedEncoderId::ComplexityField, "ComplexityField"),
            (SeedEncoderId::Structural, "Structural"),
        ];
        ids.into_iter()
            .map(|(id, label)| {
                let a = encode_seed(&in_a, id, &params, 0, 0, GRID_SIDE, GRID_SIDE).base_values;
                let b = encode_seed(&in_b, id, &params, 0, 0, GRID_SIDE, GRID_SIDE).base_values;
                (a, b, label)
            })
            .collect()
    }

    fn assert_all_identical(grids: Vec<(SeedValueGrid, SeedValueGrid, &'static str)>, kind: &str) {
        let mut failures: Vec<String> = Vec::new();
        for (a, b, label) in grids {
            let diffs = grid_diff_count(&a, &b);
            if diffs > 0 {
                failures.push(format!("{label}={diffs}"));
            }
        }
        assert!(
            failures.is_empty(),
            "{kind}: encoder grids differ for {} — behaviour-equivalent source should yield identical AST features",
            failures.join(", "),
        );
    }

    #[test]
    fn comment_only_diff_yields_identical_grid() {
        assert_all_identical(encode_all(WITH_COMMENTS, NO_COMMENTS), "comment-only diff");
    }

    // End-to-end invariance via `compute_genome_report` — the actual entry
    // point the TUI / agents hit. Catches leaks anywhere downstream of the
    // encoders (adaptive_grid_size, jitter seed, parsimony-via-tier, etc.)
    // that per-encoder tests would miss. If an agent adding comments to a
    // file ever changes the tier or generations, this test fails first.
    #[test]
    fn comment_only_diff_yields_identical_genome_report() {
        use crate::compute_genome_report;
        let p = std::path::Path::new("test.rs");
        let a = compute_genome_report(WITH_COMMENTS, p);
        let b = compute_genome_report(NO_COMMENTS, p);
        let gens_a: Vec<u32> = a
            .encoder_scores
            .iter()
            .map(|s| s.generations_survived)
            .collect();
        let gens_b: Vec<u32> = b
            .encoder_scores
            .iter()
            .map(|s| s.generations_survived)
            .collect();
        assert_eq!(
            (a.tier, &gens_a, a.grid_size),
            (b.tier, &gens_b, b.grid_size),
            "comment-only edit must not move tier / generations / grid_size",
        );
    }

    #[test]
    fn identifier_rename_yields_identical_genome_report() {
        use crate::compute_genome_report;
        let p = std::path::Path::new("test.rs");
        let a = compute_genome_report(NAMES_A, p);
        let b = compute_genome_report(NAMES_B, p);
        let gens_a: Vec<u32> = a
            .encoder_scores
            .iter()
            .map(|s| s.generations_survived)
            .collect();
        let gens_b: Vec<u32> = b
            .encoder_scores
            .iter()
            .map(|s| s.generations_survived)
            .collect();
        assert_eq!(
            (a.tier, &gens_a, a.grid_size),
            (b.tier, &gens_b, b.grid_size),
            "identifier-only rename must not move tier / generations / grid_size",
        );
    }

    // Three more behaviour-equivalent levers an agent could reach for after
    // the comment / rename / whitespace path was closed:
    //
    // - attributes — `#[derive(...)]`, `#[allow(...)]` adds AST nodes
    //   without changing behaviour. The fix: skip attribute nodes the same
    //   way we skip comments.
    // - macro arguments — `dbg!("anything you want here")` is one
    //   `macro_invocation` but the token-tree inside it expands to many
    //   AST nodes. The fix: collapse macro_invocations to one feature
    //   regardless of what's in the args.
    // - top-level item order — swapping two `fn` items is structurally
    //   equivalent in Rust but currently changes the walk order. The fix:
    //   sort top-level items by their structural signature.
    //
    // These tests fail today; closing the levers makes them pass.

    const NO_ATTRIBUTES: &str = r#"
fn compute(x: i32) -> i32 {
    let squared = x * x;
    let result = squared + 42;
    result
}

struct Wrapper {
    inner: i32,
}
"#;

    const WITH_ATTRIBUTES: &str = r#"
#[inline]
#[allow(dead_code)]
fn compute(x: i32) -> i32 {
    let squared = x * x;
    let result = squared + 42;
    result
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Wrapper {
    inner: i32,
}
"#;

    const MACRO_SHORT: &str = r#"
fn report() {
    println!("ok");
    dbg!();
}
"#;

    const MACRO_LONG: &str = r#"
fn report() {
    println!("a much longer string that an agent could swell to perturb the seed without changing what the program does at all");
    dbg!("padding", 1, 2, 3, vec![1, 2, 3, 4, 5]);
}
"#;

    const FUNCS_ORDER_A: &str = r#"
fn compute(x: i32) -> i32 {
    let squared = x * x;
    let result = squared + 42;
    result
}

fn helper(a: &str) -> usize {
    a.len()
}
"#;

    const FUNCS_ORDER_B: &str = r#"
fn helper(a: &str) -> usize {
    a.len()
}

fn compute(x: i32) -> i32 {
    let squared = x * x;
    let result = squared + 42;
    result
}
"#;

    #[test]
    fn attribute_addition_yields_identical_genome_report() {
        use crate::compute_genome_report;
        let p = std::path::Path::new("test.rs");
        let a = compute_genome_report(NO_ATTRIBUTES, p);
        let b = compute_genome_report(WITH_ATTRIBUTES, p);
        let gens_a: Vec<u32> = a
            .encoder_scores
            .iter()
            .map(|s| s.generations_survived)
            .collect();
        let gens_b: Vec<u32> = b
            .encoder_scores
            .iter()
            .map(|s| s.generations_survived)
            .collect();
        assert_eq!(
            (a.tier, &gens_a, a.grid_size),
            (b.tier, &gens_b, b.grid_size),
            "adding #[derive] / #[inline] / #[allow] must not move tier / generations / grid_size",
        );
    }

    #[test]
    fn macro_argument_change_yields_identical_genome_report() {
        use crate::compute_genome_report;
        let p = std::path::Path::new("test.rs");
        let a = compute_genome_report(MACRO_SHORT, p);
        let b = compute_genome_report(MACRO_LONG, p);
        let gens_a: Vec<u32> = a
            .encoder_scores
            .iter()
            .map(|s| s.generations_survived)
            .collect();
        let gens_b: Vec<u32> = b
            .encoder_scores
            .iter()
            .map(|s| s.generations_survived)
            .collect();
        assert_eq!(
            (a.tier, &gens_a, a.grid_size),
            (b.tier, &gens_b, b.grid_size),
            "stuffing the args of `println!` / `dbg!` must not move tier / generations / grid_size",
        );
    }

    // Strictest level: assert the encoder grids themselves are byte-
    // identical. Catches *any* drift even if the GoL simulation would have
    // averaged the difference out before reaching tier/generations.
    #[test]
    fn attribute_addition_yields_identical_grid() {
        assert_all_identical(
            encode_all(NO_ATTRIBUTES, WITH_ATTRIBUTES),
            "attribute addition",
        );
    }

    #[test]
    fn macro_argument_change_yields_identical_grid() {
        assert_all_identical(encode_all(MACRO_SHORT, MACRO_LONG), "macro argument change");
    }

    #[test]
    fn function_reorder_yields_identical_grid() {
        assert_all_identical(encode_all(FUNCS_ORDER_A, FUNCS_ORDER_B), "function reorder");
    }

    #[test]
    fn function_reorder_yields_identical_genome_report() {
        use crate::compute_genome_report;
        let p = std::path::Path::new("test.rs");
        let a = compute_genome_report(FUNCS_ORDER_A, p);
        let b = compute_genome_report(FUNCS_ORDER_B, p);
        let gens_a: Vec<u32> = a
            .encoder_scores
            .iter()
            .map(|s| s.generations_survived)
            .collect();
        let gens_b: Vec<u32> = b
            .encoder_scores
            .iter()
            .map(|s| s.generations_survived)
            .collect();
        assert_eq!(
            (a.tier, &gens_a, a.grid_size),
            (b.tier, &gens_b, b.grid_size),
            "swapping two top-level fn items must not move tier / generations / grid_size",
        );
    }

    #[test]
    fn whitespace_only_diff_yields_identical_genome_report() {
        use crate::compute_genome_report;
        let p = std::path::Path::new("test.rs");
        let a = compute_genome_report(NORMAL_WHITESPACE, p);
        let b = compute_genome_report(SHIFTED_WHITESPACE, p);
        let gens_a: Vec<u32> = a
            .encoder_scores
            .iter()
            .map(|s| s.generations_survived)
            .collect();
        let gens_b: Vec<u32> = b
            .encoder_scores
            .iter()
            .map(|s| s.generations_survived)
            .collect();
        assert_eq!(
            (a.tier, &gens_a, a.grid_size),
            (b.tier, &gens_b, b.grid_size),
            "whitespace-only reformatting must not move tier / generations / grid_size",
        );
    }

    // Lower-level invariant: the canonical AST feature vector (and its
    // hash) must be identical across all three behaviour-equivalent
    // rewrites. `significant_rows` is allowed to differ — it's positional
    // metadata reflecting which source rows host significant nodes, and
    // is intentionally excluded from the hash for that reason.
    #[test]
    fn behavior_equivalent_rewrites_yield_identical_feature_hash() {
        use super::super::encoders::ast_features::compute_ast_features;
        let cases = [
            (WITH_COMMENTS, NO_COMMENTS, "comments"),
            (NAMES_A, NAMES_B, "identifier rename"),
            (NORMAL_WHITESPACE, SHIFTED_WHITESPACE, "whitespace"),
        ];
        for (a_text, b_text, label) in cases {
            let a = compute_ast_features(a_text, path()).expect("parse A");
            let b = compute_ast_features(b_text, path()).expect("parse B");
            assert_eq!(
                (a.feature_hash, a.nodes.len()),
                (b.feature_hash, b.nodes.len()),
                "{label}: feature_hash + node count must match"
            );
        }
    }

    #[test]
    fn identifier_rename_yields_identical_grid() {
        assert_all_identical(encode_all(NAMES_A, NAMES_B), "identifier rename");
    }

    #[test]
    fn whitespace_only_diff_yields_identical_grid() {
        assert_all_identical(
            encode_all(NORMAL_WHITESPACE, SHIFTED_WHITESPACE),
            "whitespace-only diff",
        );
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
