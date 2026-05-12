//! Adversarial corpus for the genome score.
//!
//! Each pair is a behaviour-equivalent rewrite an agent might attempt in a
//! "genome retry" loop. The harness asserts `compute_genome_report_fast`
//! returns identical `(tier, generations, grid_size, recommendation metric
//! keys, parsimony shape)` for every pair.
//!
//! Fixtures sit above `GENOME_MIN_SIGNIFICANT_LINES` (~30+ sig lines each)
//! so the assertion actually traverses `run_encoders`, not the small-file
//! auto-pass. AFTER variants stay below the 0.35 comment-ratio bloat
//! threshold — a fixture that triggers parsimony bloat is no longer a
//! behaviour-equivalent rewrite.
//!
//! Extending the corpus: distill each new gaming attempt (or new encoder
//! fix's exploit) into a `<technique>_<flavour>` pair so failures point at
//! the lever immediately. Keep cases recognisable — they should look like
//! real code an agent might produce.

use std::collections::BTreeSet;
use std::path::Path;

use crate::genome_report::compute_genome_report_fast;
use crate::seed::encoders::ast_features::seed_parse;

const CASES: &[(&str, &str, &str)] = &[
    ("comments_doc_stuffing", DOC_STUFF_BEFORE, DOC_STUFF_AFTER),
    (
        "comments_inline_padding",
        INLINE_PADDING_BEFORE,
        INLINE_PADDING_AFTER,
    ),
    ("rename_idents_to_random", RENAME_BEFORE, RENAME_AFTER),
    ("whitespace_reflow", WHITESPACE_BEFORE, WHITESPACE_AFTER),
    (
        "attrs_derive_stuffing",
        DERIVE_STUFF_BEFORE,
        DERIVE_STUFF_AFTER,
    ),
    (
        "attrs_allow_stuffing",
        ALLOW_STUFF_BEFORE,
        ALLOW_STUFF_AFTER,
    ),
    ("macros_dbg_args", MACRO_BEFORE, MACRO_AFTER),
    (
        "reorder_top_level_fns",
        REORDER_FNS_BEFORE,
        REORDER_FNS_AFTER,
    ),
    (
        "reorder_top_level_items",
        REORDER_ITEMS_BEFORE,
        REORDER_ITEMS_AFTER,
    ),
    (
        "comments_block_padding",
        BLOCK_PADDING_BEFORE,
        BLOCK_PADDING_AFTER,
    ),
];

#[test]
fn seed_parse_handles_rust_grammar() {
    // Guards against silent grammar regressions that would push the entire
    // corpus into the byte-hash fallback.
    assert!(seed_parse("fn f(){}", Some(Path::new("p.rs"))).is_some());
}

#[test]
fn adversarial_corpus_genome_report_invariant() {
    let p = Path::new("test.rs");
    let mut failures: Vec<String> = Vec::new();
    for (name, before, after) in CASES {
        let a = compute_genome_report_fast(before, p);
        let b = compute_genome_report_fast(after, p);
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
        let metrics_a: BTreeSet<&str> = a
            .recommendations
            .iter()
            .map(|r| r.metric.as_str())
            .collect();
        let metrics_b: BTreeSet<&str> = b
            .recommendations
            .iter()
            .map(|r| r.metric.as_str())
            .collect();
        if a.tier != b.tier
            || gens_a != gens_b
            || a.grid_size != b.grid_size
            || metrics_a != metrics_b
            || a.parsimony.duplicate_comment_lines != b.parsimony.duplicate_comment_lines
        {
            failures.push(format!(
                "{name}: tier {:?}→{:?}, grid {}→{}, gens {:?}→{:?}, metrics {:?}→{:?}, dup_comments {}→{}",
                a.tier,
                b.tier,
                a.grid_size,
                b.grid_size,
                gens_a,
                gens_b,
                metrics_a,
                metrics_b,
                a.parsimony.duplicate_comment_lines,
                b.parsimony.duplicate_comment_lines,
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "adversarial corpus regressions:\n  {}",
        failures.join("\n  "),
    );
}

// ---------------------------------------------------------------------------
// Adversarial fixtures. Each pair is behaviour-equivalent and sized at
// ≥30 significant lines so the encoder pipeline runs over real grid space.
// AFTER variants stay below the parsimony bloat thresholds so the invariance
// assertion isn't tripped by a benign annotation pass.
// ---------------------------------------------------------------------------

const DOC_STUFF_BEFORE: &str = r#"
fn aggregate(items: &[i32]) -> i32 {
    let mut sum = 0;
    for v in items {
        sum += v;
    }
    sum
}

fn average(items: &[i32]) -> i32 {
    if items.is_empty() {
        return 0;
    }
    aggregate(items) / items.len() as i32
}

fn maximum(items: &[i32]) -> Option<i32> {
    let mut best: Option<i32> = None;
    for v in items {
        match best {
            None => best = Some(*v),
            Some(b) if *v > b => best = Some(*v),
            _ => {}
        }
    }
    best
}

fn minimum(items: &[i32]) -> Option<i32> {
    let mut best: Option<i32> = None;
    for v in items {
        match best {
            None => best = Some(*v),
            Some(b) if *v < b => best = Some(*v),
            _ => {}
        }
    }
    best
}
"#;

// Comments live BETWEEN functions, not before the first one — source_file's
// start row tracks the first non-blank content, and prepending a comment
// before fn 1 shifts source_file off function_item, breaking the row-set
// overlap and adding a phantom sig_row.
const DOC_STUFF_AFTER: &str = r#"
fn aggregate(items: &[i32]) -> i32 {
    let mut sum = 0;
    for v in items {
        sum += v;
    }
    sum
}

// Integer average; returns 0 for an empty slice.
fn average(items: &[i32]) -> i32 {
    if items.is_empty() {
        return 0;
    }
    aggregate(items) / items.len() as i32
}

// Largest element, or None for an empty slice.
fn maximum(items: &[i32]) -> Option<i32> {
    let mut best: Option<i32> = None;
    for v in items {
        match best {
            None => best = Some(*v),
            Some(b) if *v > b => best = Some(*v),
            _ => {}
        }
    }
    best
}

// Smallest element, or None for an empty slice.
fn minimum(items: &[i32]) -> Option<i32> {
    let mut best: Option<i32> = None;
    for v in items {
        match best {
            None => best = Some(*v),
            Some(b) if *v < b => best = Some(*v),
            _ => {}
        }
    }
    best
}
"#;

const INLINE_PADDING_BEFORE: &str = r#"
fn run(x: i32) -> i32 {
    let a = x * 2;
    let b = a + 1;
    let c = b * b;
    let d = c - x;
    let e = d.saturating_abs();
    e
}

fn pipeline(data: &[i32]) -> Vec<i32> {
    let mut out = Vec::new();
    for v in data {
        let mapped = run(*v);
        if mapped > 0 {
            out.push(mapped);
        }
    }
    out
}

fn classify(n: i32) -> &'static str {
    if n < -10 {
        "low"
    } else if n < 10 {
        "mid"
    } else {
        "high"
    }
}

fn pair(a: i32, b: i32) -> (i32, i32) {
    (a + b, a - b)
}
"#;

// Inline trailing comments don't satisfy `is_comment_line` (which requires the
// trimmed line to start with `//`), so they don't move comment_ratio — but
// they DO change byte-length and AST-row spacing if the encoder is leaky.
const INLINE_PADDING_AFTER: &str = r#"
fn run(x: i32) -> i32 {
    let a = x * 2; // double
    let b = a + 1; // offset
    let c = b * b; // square
    let d = c - x; // subtract
    let e = d.saturating_abs(); // absolute
    e // explicit
}

fn pipeline(data: &[i32]) -> Vec<i32> {
    let mut out = Vec::new(); // accumulator
    for v in data {
        let mapped = run(*v); // transform
        if mapped > 0 {
            out.push(mapped); // keep positives
        }
    }
    out
}

fn classify(n: i32) -> &'static str {
    if n < -10 {
        "low"
    } else if n < 10 {
        "mid"
    } else {
        "high"
    }
}

fn pair(a: i32, b: i32) -> (i32, i32) {
    (a + b, a - b)
}
"#;

const RENAME_BEFORE: &str = r#"
fn compute(value: i32, scale: i32) -> i32 {
    let intermediate = value * scale;
    let final_value = intermediate + 1;
    final_value
}

fn apply(value: i32, scale: i32, offset: i32) -> i32 {
    let scaled = compute(value, scale);
    scaled + offset
}

fn batch(values: &[i32], scale: i32) -> Vec<i32> {
    let mut out = Vec::with_capacity(values.len());
    for v in values {
        out.push(compute(*v, scale));
    }
    out
}

fn fold(values: &[i32], scale: i32) -> i32 {
    let mut acc = 0;
    for v in values {
        acc = compute(acc + v, scale);
    }
    acc
}

fn pipeline(values: &[i32], scale: i32, offset: i32) -> i32 {
    let batched = batch(values, scale);
    let folded = fold(&batched, scale);
    apply(folded, scale, offset)
}
"#;

const RENAME_AFTER: &str = r#"
fn xqz(zz: i32, ww: i32) -> i32 {
    let qq = zz * ww;
    let rr = qq + 1;
    rr
}

fn yzy(zz: i32, ww: i32, oo: i32) -> i32 {
    let ss = xqz(zz, ww);
    ss + oo
}

fn bbb(vv: &[i32], ww: i32) -> Vec<i32> {
    let mut tt = Vec::with_capacity(vv.len());
    for u in vv {
        tt.push(xqz(*u, ww));
    }
    tt
}

fn ffd(vv: &[i32], ww: i32) -> i32 {
    let mut kk = 0;
    for u in vv {
        kk = xqz(kk + u, ww);
    }
    kk
}

fn ppl(vv: &[i32], ww: i32, oo: i32) -> i32 {
    let nn = bbb(vv, ww);
    let mm = ffd(&nn, ww);
    yzy(mm, ww, oo)
}
"#;

const WHITESPACE_BEFORE: &str = r#"
fn pipeline(input: &str) -> usize {
    let trimmed = input.trim();
    let lines = trimmed.lines().count();
    lines
}

fn words(input: &str) -> usize {
    input.split_whitespace().count()
}

fn bytes(input: &str) -> usize {
    input.len()
}

fn chars(input: &str) -> usize {
    input.chars().count()
}

fn stats(input: &str) -> (usize, usize, usize, usize) {
    let l = pipeline(input);
    let w = words(input);
    let b = bytes(input);
    let c = chars(input);
    (l, w, b, c)
}

fn ratio(input: &str) -> f32 {
    let (l, w, _, _) = stats(input);
    if l == 0 {
        0.0
    } else {
        w as f32 / l as f32
    }
}
"#;

// Same AST, aggressively reformatted: padded operators / parens, double-blank
// separators between statements, exaggerated indentation.
const WHITESPACE_AFTER: &str = "
fn pipeline( input : &str ) -> usize {

  let trimmed = input . trim() ;


  let lines = trimmed . lines() . count() ;


  lines
}

fn words( input : &str ) -> usize {

  input . split_whitespace() . count()
}

fn bytes( input : &str ) -> usize {

  input . len()
}

fn chars( input : &str ) -> usize {

  input . chars() . count()
}

fn stats( input : &str ) -> ( usize , usize , usize , usize ) {

  let l = pipeline( input ) ;
  let w = words( input ) ;
  let b = bytes( input ) ;
  let c = chars( input ) ;
  ( l , w , b , c )
}

fn ratio( input : &str ) -> f32 {

  let ( l , w , _ , _ ) = stats( input ) ;

  if l == 0 {
    0.0
  } else {
    w as f32 / l as f32
  }
}
";

const DERIVE_STUFF_BEFORE: &str = r#"
struct Wrapper {
    inner: i32,
}

struct Container {
    values: Vec<Wrapper>,
}

struct Index {
    by_id: Vec<usize>,
    by_value: Vec<i32>,
}

fn build() -> Container {
    Container { values: Vec::new() }
}

fn push_wrapper(c: &mut Container, value: i32) {
    c.values.push(Wrapper { inner: value });
}

fn build_index(c: &Container) -> Index {
    let mut idx = Index {
        by_id: Vec::with_capacity(c.values.len()),
        by_value: Vec::with_capacity(c.values.len()),
    };
    for (i, w) in c.values.iter().enumerate() {
        idx.by_id.push(i);
        idx.by_value.push(w.inner);
    }
    idx
}

fn count(c: &Container) -> usize {
    c.values.len()
}
"#;

const DERIVE_STUFF_AFTER: &str = r#"
struct Wrapper {
    inner: i32,
}

#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
struct Container {
    values: Vec<Wrapper>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Index {
    by_id: Vec<usize>,
    by_value: Vec<i32>,
}

#[inline]
#[must_use]
fn build() -> Container {
    Container { values: Vec::new() }
}

#[inline]
fn push_wrapper(c: &mut Container, value: i32) {
    c.values.push(Wrapper { inner: value });
}

#[must_use]
fn build_index(c: &Container) -> Index {
    let mut idx = Index {
        by_id: Vec::with_capacity(c.values.len()),
        by_value: Vec::with_capacity(c.values.len()),
    };
    for (i, w) in c.values.iter().enumerate() {
        idx.by_id.push(i);
        idx.by_value.push(w.inner);
    }
    idx
}

#[inline]
fn count(c: &Container) -> usize {
    c.values.len()
}
"#;

const ALLOW_STUFF_BEFORE: &str = r#"
fn process(items: &[String]) -> Vec<usize> {
    items.iter().map(|s| s.len()).collect()
}

fn shorten(items: &[String], max: usize) -> Vec<String> {
    items
        .iter()
        .map(|s| if s.len() > max { s[..max].to_string() } else { s.clone() })
        .collect()
}

fn lengths(items: &[String]) -> (usize, usize) {
    let mut min = usize::MAX;
    let mut max = 0;
    for s in items {
        let n = s.len();
        if n < min {
            min = n;
        }
        if n > max {
            max = n;
        }
    }
    (min, max)
}

fn total_bytes(items: &[String]) -> usize {
    items.iter().map(|s| s.len()).sum()
}

fn average_length(items: &[String]) -> f32 {
    if items.is_empty() {
        return 0.0;
    }
    total_bytes(items) as f32 / items.len() as f32
}
"#;

const ALLOW_STUFF_AFTER: &str = r#"
fn process(items: &[String]) -> Vec<usize> {
    items.iter().map(|s| s.len()).collect()
}

#[allow(clippy::needless_collect)]
fn shorten(items: &[String], max: usize) -> Vec<String> {
    items
        .iter()
        .map(|s| if s.len() > max { s[..max].to_string() } else { s.clone() })
        .collect()
}

#[allow(clippy::collapsible_if)]
fn lengths(items: &[String]) -> (usize, usize) {
    let mut min = usize::MAX;
    let mut max = 0;
    for s in items {
        let n = s.len();
        if n < min {
            min = n;
        }
        if n > max {
            max = n;
        }
    }
    (min, max)
}

#[allow(clippy::redundant_closure)]
fn total_bytes(items: &[String]) -> usize {
    items.iter().map(|s| s.len()).sum()
}

#[allow(clippy::cast_precision_loss)]
fn average_length(items: &[String]) -> f32 {
    if items.is_empty() {
        return 0.0;
    }
    total_bytes(items) as f32 / items.len() as f32
}
"#;

const MACRO_BEFORE: &str = r#"
fn observe(label: &str, n: i32) -> i32 {
    println!("{}", label);
    dbg!(n);
    n
}

fn report(label: &str, n: i32) -> i32 {
    eprintln!("{}", label);
    n + 1
}

fn trace(label: &str, n: i32) -> i32 {
    println!("{}: {}", label, n);
    n
}

fn count(label: &str, items: &[i32]) -> usize {
    eprintln!("counting {}", label);
    items.len()
}

fn dump(label: &str, items: &[i32]) {
    println!("{}", label);
    for v in items {
        dbg!(v);
    }
}

fn pipeline(label: &str, items: &[i32]) -> i32 {
    let n = count(label, items);
    let acc = items.iter().sum();
    observe(label, acc + n as i32)
}
"#;

// Macro arg-strings extended (string literals only — no new identifiers).
// Macro contents collapse to a single Expression in AST features, so the
// encoder is invariant; the recommendation walker is identifier-counting,
// so adding NEW identifier tokens (e.g. `pid`, `thread`) would shift
// uniqueness while string-only expansion keeps it stable.
const MACRO_AFTER: &str = r#"
fn observe(label: &str, n: i32) -> i32 {
    println!("[trace] {} value={} :: end of trace line, padded with extra text", label, n);
    dbg!(n);
    n
}

fn report(label: &str, n: i32) -> i32 {
    eprintln!("[trace] {} :: longer descriptive padding here", label);
    n + 1
}

fn trace(label: &str, n: i32) -> i32 {
    println!("[trace] {}: {} :: extended trace padding for the line", label, n);
    n
}

fn count(label: &str, items: &[i32]) -> usize {
    eprintln!("[counting] {} :: extra padding for the trace line here", label);
    items.len()
}

fn dump(label: &str, items: &[i32]) {
    println!("[dumping] {} :: more descriptive padding text", label);
    for v in items {
        dbg!(v);
    }
}

fn pipeline(label: &str, items: &[i32]) -> i32 {
    let n = count(label, items);
    let acc = items.iter().sum();
    observe(label, acc + n as i32)
}
"#;

const REORDER_FNS_BEFORE: &str = r#"
fn alpha(x: i32) -> i32 {
    let y = x + 1;
    y * 2
}

fn beta(s: &str) -> usize {
    s.len() + 1
}

fn gamma(n: u64) -> u64 {
    if n == 0 { 1 } else { n * n }
}

fn delta(items: &[i32]) -> i32 {
    let mut acc = 0;
    for v in items {
        acc += v;
    }
    acc
}

fn epsilon(items: &[i32]) -> i32 {
    let mut acc = 1;
    for v in items {
        acc *= v;
    }
    acc
}

fn zeta(items: &[i32]) -> Option<i32> {
    let mut best: Option<i32> = None;
    for v in items {
        if best.map_or(true, |b| *v > b) {
            best = Some(*v);
        }
    }
    best
}
"#;

const REORDER_FNS_AFTER: &str = r#"
fn zeta(items: &[i32]) -> Option<i32> {
    let mut best: Option<i32> = None;
    for v in items {
        if best.map_or(true, |b| *v > b) {
            best = Some(*v);
        }
    }
    best
}

fn gamma(n: u64) -> u64 {
    if n == 0 { 1 } else { n * n }
}

fn alpha(x: i32) -> i32 {
    let y = x + 1;
    y * 2
}

fn epsilon(items: &[i32]) -> i32 {
    let mut acc = 1;
    for v in items {
        acc *= v;
    }
    acc
}

fn beta(s: &str) -> usize {
    s.len() + 1
}

fn delta(items: &[i32]) -> i32 {
    let mut acc = 0;
    for v in items {
        acc += v;
    }
    acc
}
"#;

// Structs interleaved with fns so neither BEFORE nor AFTER has a 10-line
// window of consecutive struct definitions — `token_entropy` is windowed,
// and clustered struct defs produce a low-entropy window that fires on one
// ordering and not the other.
const REORDER_ITEMS_BEFORE: &str = r#"
struct A {
    x: i32,
}

fn use_a(a: A) -> usize {
    a.x as usize
}

struct B {
    y: String,
}

fn use_b(b: B) -> usize {
    b.y.len()
}

struct C {
    flag: bool,
    counter: u32,
}

fn use_c(c: C) -> usize {
    if c.flag {
        c.counter as usize
    } else {
        0
    }
}

fn use_them(a: A, b: B, c: C) -> usize {
    use_a(a) + use_b(b) + use_c(c)
}

fn build_b(s: String) -> B {
    B { y: s }
}

fn build_c(flag: bool, counter: u32) -> C {
    C { flag, counter }
}
"#;

const REORDER_ITEMS_AFTER: &str = r#"
fn build_b(s: String) -> B {
    B { y: s }
}

struct B {
    y: String,
}

fn use_them(a: A, b: B, c: C) -> usize {
    use_a(a) + use_b(b) + use_c(c)
}

struct A {
    x: i32,
}

fn build_c(flag: bool, counter: u32) -> C {
    C { flag, counter }
}

fn use_c(c: C) -> usize {
    if c.flag {
        c.counter as usize
    } else {
        0
    }
}

struct C {
    flag: bool,
    counter: u32,
}

fn use_a(a: A) -> usize {
    a.x as usize
}

fn use_b(b: B) -> usize {
    b.y.len()
}
"#;

const BLOCK_PADDING_BEFORE: &str = r#"
fn classify(n: i32) -> &'static str {
    if n < 0 {
        "negative"
    } else if n == 0 {
        "zero"
    } else {
        "positive"
    }
}

fn bucket(n: i32) -> i32 {
    if n < 10 {
        0
    } else if n < 100 {
        1
    } else if n < 1000 {
        2
    } else {
        3
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

fn label(n: i32) -> String {
    let kind = classify(n);
    let level = bucket(n.abs());
    format!("{}-{}", kind, level)
}

fn pipeline(values: &[i32]) -> Vec<String> {
    values.iter().map(|v| label(*v)).collect()
}
"#;

// Same control flow, padded with block comments. Block-comment lines DO
// satisfy `is_comment_line`, so AFTER's comment_ratio sits a few points above
// BEFORE's — but well below the 0.35 bloat threshold thanks to the size of
// the underlying code body.
const BLOCK_PADDING_AFTER: &str = r#"
fn classify(n: i32) -> &'static str {
    /* negative branch */
    if n < 0 {
        "negative"
    } else if n == 0 {
        "zero"
    } else {
        "positive"
    }
}

fn bucket(n: i32) -> i32 {
    /* tens / hundreds / thousands */
    if n < 10 {
        0
    } else if n < 100 {
        1
    } else if n < 1000 {
        2
    } else {
        3
    }
}

fn signum(n: i32) -> i32 {
    /* canonical sign */
    if n < 0 {
        -1
    } else if n == 0 {
        0
    } else {
        1
    }
}

fn label(n: i32) -> String {
    /* compose kind + level */
    let kind = classify(n);
    let level = bucket(n.abs());
    format!("{}-{}", kind, level)
}

fn pipeline(values: &[i32]) -> Vec<String> {
    values.iter().map(|v| label(*v)).collect()
}
"#;
