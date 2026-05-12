//! Property-based invariance tests for the genome score.
//!
//! Generates random *behaviour-equivalent* rewrites of a small corpus of base
//! source files and asserts every variant produces the same
//! `compute_genome_report_fast` as the original. Catches gaming levers nobody
//! enumerated — unlike the adversarial corpus, which only covers known
//! patterns.
//!
//! Hand-rolled rather than using `proptest` to avoid the dev-dep +
//! `Cargo.lock` churn. The shape is the same: deterministic PRNG →
//! composable transforms → invariance assertion over N iterations. Seeds
//! ride in the failure message so any regression is reproducible.
//!
//! Bases sit above `GENOME_MIN_SIGNIFICANT_LINES` (~30+ sig lines) so the
//! encoder pipeline actually runs.

use std::collections::BTreeSet;
use std::path::Path;

use crate::genome_report::compute_genome_report_fast;
use nit_utils::rng::SplitMix64;

const BASES: &[&str] = &[BASE_SIMPLE, BASE_BRANCHY, BASE_NESTED, BASE_MULTI_ITEM];

const BASE_SIMPLE: &str = r#"
fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn mul(a: i32, b: i32) -> i32 {
    a * b
}

fn sub(a: i32, b: i32) -> i32 {
    a - b
}

fn safe_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        0
    } else {
        a / b
    }
}

fn power(base: i32, exp: u32) -> i32 {
    let mut acc = 1;
    let mut e = exp;
    while e > 0 {
        acc *= base;
        e -= 1;
    }
    acc
}

fn sum_of_squares(values: &[i32]) -> i32 {
    let mut acc = 0;
    for v in values {
        acc += v * v;
    }
    acc
}
"#;

const BASE_BRANCHY: &str = r#"
fn classify(n: i32) -> &'static str {
    if n < 0 {
        "negative"
    } else if n == 0 {
        "zero"
    } else if n < 10 {
        "small"
    } else if n < 100 {
        "medium"
    } else {
        "large"
    }
}

fn signum(n: i32) -> i32 {
    if n < 0 {
        -1
    } else if n == 0 {
        0
    } else {
        1
    }
}

fn band(n: i32) -> i32 {
    if n < -100 {
        -2
    } else if n < 0 {
        -1
    } else if n < 100 {
        0
    } else if n < 1000 {
        1
    } else {
        2
    }
}

fn priority(n: i32) -> u8 {
    let kind = classify(n);
    let sign = signum(n);
    if sign < 0 || kind == "large" {
        1
    } else {
        0
    }
}
"#;

const BASE_NESTED: &str = r#"
fn process(items: &[i32], threshold: i32) -> Vec<i32> {
    let mut out = Vec::new();
    for v in items {
        if *v > threshold {
            for other in items {
                if other != v {
                    out.push(v + other);
                }
            }
        }
    }
    out
}

fn cross_product(a: &[i32], b: &[i32]) -> Vec<i32> {
    let mut out = Vec::with_capacity(a.len() * b.len());
    for x in a {
        for y in b {
            out.push(x * y);
        }
    }
    out
}

fn matrix_sum(rows: &[Vec<i32>]) -> i32 {
    let mut acc = 0;
    for row in rows {
        for v in row {
            acc += v;
        }
    }
    acc
}

fn diagonal_above(rows: &[Vec<i32>], threshold: i32) -> usize {
    let mut count = 0;
    for (i, row) in rows.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            if i == j && *v > threshold {
                count += 1;
            }
        }
    }
    count
}
"#;

const BASE_MULTI_ITEM: &str = r#"
struct Point {
    x: i32,
    y: i32,
}

struct Range {
    lo: i32,
    hi: i32,
}

struct Box2 {
    p: Point,
    q: Point,
}

fn midpoint(r: &Range) -> i32 {
    (r.lo + r.hi) / 2
}

fn distance(a: &Point, b: &Point) -> i32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    dx * dx + dy * dy
}

fn box_width(b: &Box2) -> i32 {
    (b.q.x - b.p.x).abs()
}

fn box_height(b: &Box2) -> i32 {
    (b.q.y - b.p.y).abs()
}

fn box_area(b: &Box2) -> i32 {
    box_width(b) * box_height(b)
}

fn contains(r: &Range, n: i32) -> bool {
    n >= r.lo && n <= r.hi
}
"#;

const ITERATIONS: usize = 64;
const MAX_TRANSFORMS_PER_VARIANT: u32 = 8;

#[test]
fn random_behaviour_equivalent_rewrites_yield_identical_report() {
    let p = Path::new("test.rs");
    let mut failures: Vec<String> = Vec::new();

    for run_idx in 0..ITERATIONS {
        // Deterministic seed: failure messages quote it so any regression is
        // reproducible from the assertion text alone.
        let seed = 0xc0ffee_u64.wrapping_add((run_idx as u64).wrapping_mul(0x9e3779b97f4a7c15));
        let mut rng = SplitMix64::new(seed);

        let base = BASES[(rng.next_u64() as usize) % BASES.len()];
        let variant = apply_random_transforms(base, &mut rng);

        let original = compute_genome_report_fast(base, p);
        let mutated = compute_genome_report_fast(&variant, p);
        let gens_o: Vec<u32> = original
            .encoder_scores
            .iter()
            .map(|s| s.generations_survived)
            .collect();
        let gens_m: Vec<u32> = mutated
            .encoder_scores
            .iter()
            .map(|s| s.generations_survived)
            .collect();
        // `token_entropy` and `nesting_depth` are computed over fixed-size
        // row windows. Random comment / attribute insertion shifts those
        // windows, so a metric that fires on the base may not fire on the
        // variant (or vice versa) without any meaningful structural change.
        // The curated adversarial corpus controls for this; the random
        // generator cannot.
        let metrics_o: BTreeSet<&str> = stable_metric_keys(&original.recommendations);
        let metrics_m: BTreeSet<&str> = stable_metric_keys(&mutated.recommendations);
        if original.tier != mutated.tier
            || gens_o != gens_m
            || original.grid_size != mutated.grid_size
            || metrics_o != metrics_m
        {
            failures.push(format!(
                "seed=0x{seed:x}: tier {:?}→{:?}, grid {}→{}, gens {:?}→{:?}, metrics {:?}→{:?}\nvariant:\n{}",
                original.tier,
                mutated.tier,
                original.grid_size,
                mutated.grid_size,
                gens_o,
                gens_m,
                metrics_o,
                metrics_m,
                variant,
            ));
            if failures.len() >= 3 {
                break;
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} random invariance violations:\n---\n{}",
        failures.len(),
        failures.join("\n---\n"),
    );
}

fn stable_metric_keys(recs: &[crate::genome_report::GenomeRecommendation]) -> BTreeSet<&str> {
    const ROW_WINDOW_METRICS: &[&str] = &["token_entropy", "nesting_depth"];
    recs.iter()
        .map(|r| r.metric.as_str())
        .filter(|m| !ROW_WINDOW_METRICS.contains(m))
        .collect()
}

// ---------------------------------------------------------------------------
// Transforms. Each preserves program behaviour.
// ---------------------------------------------------------------------------

fn apply_random_transforms(base: &str, rng: &mut SplitMix64) -> String {
    let mut text = base.to_string();
    let count = (rng.next_u64() as u32 % MAX_TRANSFORMS_PER_VARIANT) + 1;
    for _ in 0..count {
        // `double_blank_lines` is intentionally absent: repeated application
        // creates pathological row gaps that trip windowed recommendations
        // (token_entropy, nesting_depth) without representing a realistic
        // agent attack — no agent reformats blank lines geometrically.
        text = match rng.next_u64() % 6 {
            0 => insert_line_comment(&text, rng),
            1 => insert_block_comment(&text, rng),
            2 => append_trailing_inline_comment(&text, rng),
            3 => rename_identifier(&text, rng),
            4 => prepend_attribute_to_random_item(&text, rng),
            5 => extend_string_literal(&text, rng),
            _ => unreachable!(),
        };
    }
    text
}

fn insert_line_comment(text: &str, rng: &mut SplitMix64) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return text.into();
    }
    let pick = (rng.next_u64() as usize) % lines.len();
    let mut out = String::with_capacity(text.len() + 64);
    for (i, line) in lines.iter().enumerate() {
        if i == pick {
            out.push_str("// ");
            out.push_str(&random_word(rng));
            out.push(' ');
            out.push_str(&random_word(rng));
            out.push('\n');
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn insert_block_comment(text: &str, rng: &mut SplitMix64) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return text.into();
    }
    let pick = (rng.next_u64() as usize) % lines.len();
    let mut out = String::with_capacity(text.len() + 128);
    for (i, line) in lines.iter().enumerate() {
        if i == pick {
            out.push_str("/* ");
            out.push_str(&random_word(rng));
            out.push(' ');
            out.push_str(&random_word(rng));
            out.push_str("\n   ");
            out.push_str(&random_word(rng));
            out.push_str(" */\n");
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn append_trailing_inline_comment(text: &str, rng: &mut SplitMix64) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return text.into();
    }
    let pick = (rng.next_u64() as usize) % lines.len();
    let trimmed = lines[pick].trim_end();
    if trimmed.is_empty() {
        return text.into();
    }
    let mut out = String::with_capacity(text.len() + 64);
    for (i, line) in lines.iter().enumerate() {
        if i == pick {
            out.push_str(line);
            out.push_str(" // ");
            out.push_str(&random_word(rng));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

// Renames every occurrence of one identifier to a fresh one. UTF-8-safe:
// iterates over `char_indices()` rather than byte-stepping, so multibyte
// content (none in the current bases, but a future fixture could include it)
// can't split a char in half.
fn rename_identifier(text: &str, rng: &mut SplitMix64) -> String {
    const CANDIDATES: &[&str] = &[
        "a",
        "b",
        "x",
        "y",
        "n",
        "v",
        "items",
        "out",
        "threshold",
        "lo",
        "hi",
        "dx",
        "dy",
        "midpoint",
        "distance",
        "process",
        "classify",
    ];
    let from = CANDIDATES[(rng.next_u64() as usize) % CANDIDATES.len()];
    let to = format!("z{:x}", rng.next_u64() & 0xffff);
    let bytes = text.as_bytes();
    let from_bytes = from.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (i, ch) in text.char_indices() {
        if i < cursor {
            continue;
        }
        if matches_word(bytes, i, from_bytes) {
            out.push_str(&to);
            cursor = i + from_bytes.len();
        } else {
            out.push(ch);
            cursor = i + ch.len_utf8();
        }
    }
    out
}

fn matches_word(haystack: &[u8], i: usize, needle: &[u8]) -> bool {
    if i + needle.len() > haystack.len() {
        return false;
    }
    if &haystack[i..i + needle.len()] != needle {
        return false;
    }
    let before_ok = i == 0 || !is_ident_byte(haystack[i - 1]);
    let after_ok = i + needle.len() == haystack.len() || !is_ident_byte(haystack[i + needle.len()]);
    before_ok && after_ok
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn prepend_attribute_to_random_item(text: &str, rng: &mut SplitMix64) -> String {
    const ATTRS: &[&str] = &[
        "#[inline]",
        "#[allow(dead_code)]",
        "#[allow(unused_variables)]",
        "#[derive(Clone, Debug)]",
        "#[must_use]",
        "#[cold]",
    ];
    let pick_attr = ATTRS[(rng.next_u64() as usize) % ATTRS.len()];

    let lines: Vec<&str> = text.lines().collect();
    let item_starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| {
            let t = l.trim_start();
            t.starts_with("fn ") || t.starts_with("struct ") || t.starts_with("enum ")
        })
        .map(|(i, _)| i)
        .collect();
    if item_starts.is_empty() {
        return text.into();
    }
    let pick = item_starts[(rng.next_u64() as usize) % item_starts.len()];

    let mut out = String::with_capacity(text.len() + pick_attr.len() + 2);
    for (i, line) in lines.iter().enumerate() {
        if i == pick {
            out.push_str(pick_attr);
            out.push('\n');
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

// Extending a string literal changes byte length but not AST shape — the
// `string_literal` node is one feature regardless of contents.
fn extend_string_literal(text: &str, rng: &mut SplitMix64) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let padding = format!(" {}", random_word(rng));
    let mut emitted_pad = false;
    for (idx, ch) in text.char_indices() {
        out.push(ch);
        if ch == '"' && !is_escaped(&text[..idx]) {
            if in_string {
                if !emitted_pad {
                    out.pop();
                    out.push_str(&padding);
                    out.push('"');
                    emitted_pad = true;
                }
                in_string = false;
            } else {
                in_string = true;
            }
        }
    }
    out
}

// `is_escaped` checks the immediate backslash run before the quote — the
// classic "even = literal quote, odd = escaped quote" rule. The current
// bases use no raw strings (`r"..."`), so this is sound; if a fixture ever
// adds one, the rule would need to special-case `r#"..."#` boundaries.
fn is_escaped(prefix: &str) -> bool {
    let bytes = prefix.as_bytes();
    let mut backslashes = 0usize;
    for b in bytes.iter().rev() {
        if *b == b'\\' {
            backslashes += 1;
        } else {
            break;
        }
    }
    backslashes % 2 == 1
}

fn random_word(rng: &mut SplitMix64) -> String {
    const WORDS: &[&str] = &[
        "alpha", "beta", "gamma", "delta", "tmp", "note", "phase", "step", "var", "stub", "todo",
        "fix", "later", "see", "callers", "ctx",
    ];
    WORDS[(rng.next_u64() as usize) % WORDS.len()].to_string()
}
