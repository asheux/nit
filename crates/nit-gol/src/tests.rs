use crate::{
    analyze::evaluate_rule,
    attractor::{AttractorConfig, AttractorDetector, AttractorEvent, AutoStopPolicy},
    grid::EdgeMode,
    step::step,
    Grid, Rule,
};

// ── Fixture helpers ─────────────────────────────────────────────────

/// Conway period-1 still life: a 2x2 block in the top-left quadrant.
fn make_block() -> Grid {
    let mut grid = Grid::new(4, 4);
    grid.set(1, 1, true);
    grid.set(1, 2, true);
    grid.set(2, 1, true);
    grid.set(2, 2, true);
    grid
}

/// Conway period-2 oscillator: a horizontal 3-cell line.
fn make_blinker() -> Grid {
    let mut grid = Grid::new(5, 5);
    grid.set(2, 1, true);
    grid.set(2, 2, true);
    grid.set(2, 3, true);
    grid
}

fn make_repeat_detector(max_history: usize) -> AttractorDetector {
    AttractorDetector::new(AttractorConfig {
        policy: AutoStopPolicy::Repeat,
        max_history,
        confirm_on_repeat: true,
    })
}

// ── Rule parsing ────────────────────────────────────────────────────

/// Parse B36/S23 and verify the canonical round-trip.
#[test]
fn parse_rule_roundtrip() {
    let rule = Rule::parse("B36/S23").expect("parse");
    assert_eq!(rule.to_string(), "B36/S23");
}

/// Individual bits of the birth and survival masks line up with the digits.
#[test]
fn parse_rule_masks() {
    let rule = Rule::parse("B3/S23").expect("parse");
    assert_eq!(rule.births_mask(), 1 << 3);
    assert_eq!(rule.survives_mask(), (1 << 2) | (1 << 3));
}

/// An empty survive section parses with a zero survive mask.
#[test]
fn parse_rule_empty_survive() {
    let rule = Rule::parse("B2/S").expect("parse");
    assert_eq!(rule.births_mask(), 1 << 2);
    assert_eq!(rule.survives_mask(), 0);
}

/// Malformed rule strings all fail to parse.
#[test]
fn parse_rule_invalid_cases() {
    let invalid = ["B9/S23", "B3/S2x", "B3//S23", "B3/23", "B/S"];
    for text in invalid {
        assert!(Rule::parse(text).is_err(), "expected invalid: {text}");
    }
}

// ── Grid evolution ──────────────────────────────────────────────────

/// A 2x2 block is a period-1 still life under Conway's rules.
#[test]
fn block_still_life() {
    let grid = make_block();
    let next = step(&grid, Rule::conway(), EdgeMode::Dead);
    assert_eq!(grid, next);
}

/// A 3-cell line oscillates with period 2 (the blinker).
#[test]
fn blinker_oscillator() {
    let grid = make_blinker();
    let rule = Rule::conway();
    let next = step(&grid, rule, EdgeMode::Dead);
    assert!(next.get(1, 2));
    assert!(next.get(2, 2));
    assert!(next.get(3, 2));
    let next2 = step(&next, rule, EdgeMode::Dead);
    assert_eq!(grid, next2);
}

/// The rule evaluator should detect the blinker's period-2 cycle.
#[test]
fn evaluate_rule_detects_period() {
    let grid = make_blinker();
    let eval = evaluate_rule(&grid, Rule::conway(), EdgeMode::Dead, 10);
    assert_eq!(eval.period, Some(2));
    assert!(eval.score > 0.0);
}

/// A glider translates one cell down-right every 4 generations.
#[test]
fn glider_moves_down_right() {
    let mut grid = Grid::new(6, 6);
    grid.set(1, 0, true);
    grid.set(2, 1, true);
    grid.set(0, 2, true);
    grid.set(1, 2, true);
    grid.set(2, 2, true);
    let rule = Rule::conway();
    let mut next = grid.clone();
    for _ in 0..4 {
        next = step(&next, rule, EdgeMode::Dead);
    }
    assert!(next.get(2, 1));
    assert!(next.get(3, 2));
    assert!(next.get(1, 3));
    assert!(next.get(2, 3));
    assert!(next.get(3, 3));
}

// ── Grid hashing ────────────────────────────────────────────────────

/// Flipping a cell changes the grid hash.
#[test]
fn grid_hash_changes() {
    let mut grid = Grid::new(3, 3);
    let h1 = grid.hash();
    grid.set(1, 1, true);
    let h2 = grid.hash();
    assert_ne!(h1, h2);
}

// ── RLE encoding ────────────────────────────────────────────────────

/// Header fields present and terminator correct.
#[test]
fn rle_basic_sanity() {
    let mut grid = Grid::new(3, 3);
    grid.set(1, 1, true);
    let rle = crate::snapshot::encode_rle(&grid, Rule::conway());
    assert!(rle.contains("x = 3"));
    assert!(rle.contains("y = 3"));
    assert!(rle.contains("rule = B3/S23"));
    assert!(rle.ends_with('!'));
    assert!(rle.contains('o'));
}

/// Exact byte-level RLE output for a 2x2 grid.
#[test]
fn rle_2x2_exact() {
    let mut grid = Grid::new(2, 2);
    grid.set(0, 0, true);
    grid.set(1, 0, true);
    grid.set(1, 1, true);
    let mut out = Vec::new();
    crate::snapshot::write_rle(&mut out, &grid, Rule::conway()).unwrap();
    let rle = String::from_utf8(out).unwrap();
    assert_eq!(rle, "x = 2, y = 2, rule = B3/S23\n2o$\nbo!");
}

/// Exact byte-level RLE output for a sparse 5x5 grid.
#[test]
fn rle_5x5_exact() {
    let mut grid = Grid::new(5, 5);
    grid.set(2, 2, true);
    let mut out = Vec::new();
    crate::snapshot::write_rle(&mut out, &grid, Rule::conway()).unwrap();
    let rle = String::from_utf8(out).unwrap();
    assert_eq!(rle, "x = 5, y = 5, rule = B3/S23\n5b$\n5b$\n2bo2b$\n5b$\n5b!");
}

// ── Attractor detection ─────────────────────────────────────────────

/// A still-life block emits a FixedPoint event on the first observe.
#[test]
fn attractor_still_life_fixed_point() {
    let grid = make_block();
    let rule = Rule::conway();
    let edge = EdgeMode::Dead;
    let mut detector = make_repeat_detector(128);
    detector.seed(&grid, 0, rule, edge);
    let next = step(&grid, rule, edge);
    let event = detector.observe(&grid, &next, 1, rule, edge);
    assert_eq!(event, Some(AttractorEvent::FixedPoint { gen: 1 }));
}

/// A blinker emits a Cycle event with period 2.
#[test]
fn attractor_blinker_cycle() {
    let grid = make_blinker();
    let rule = Rule::conway();
    let edge = EdgeMode::Dead;
    let mut detector = make_repeat_detector(128);
    detector.seed(&grid, 0, rule, edge);
    let next = step(&grid, rule, edge);
    let event1 = detector.observe(&grid, &next, 1, rule, edge);
    assert!(event1.is_none());
    let next2 = step(&next, rule, edge);
    let event2 = detector.observe(&next, &next2, 2, rule, edge);
    assert_eq!(
        event2,
        Some(AttractorEvent::Cycle {
            gen: 2,
            first_seen: 0,
            period: 2,
            transient: 0
        })
    );
}

/// An empty grid is a fixed point — Conway has no spontaneous births.
#[test]
fn attractor_empty_grid_fixed_point() {
    let grid = Grid::new(3, 3);
    let rule = Rule::conway();
    let edge = EdgeMode::Dead;
    let mut detector = make_repeat_detector(64);
    detector.seed(&grid, 0, rule, edge);
    let next = step(&grid, rule, edge);
    let event = detector.observe(&grid, &next, 1, rule, edge);
    assert_eq!(event, Some(AttractorEvent::FixedPoint { gen: 1 }));
}

/// Two grids with the same fingerprint but different secondary hashes
/// are not treated as a cycle (collision guard).
#[test]
fn attractor_hash_collision_guard() {
    let mut grid_a = Grid::new(2, 2);
    grid_a.set(0, 0, true);
    let mut grid_b = Grid::new(2, 2);
    grid_b.set(1, 1, true);
    let mut detector = make_repeat_detector(64);
    let fp = AttractorDetector::test_fingerprint(0xdead_beef_dead_beef_dead_beef_dead_beef);
    detector.seed_with_fingerprint(0, fp, Some(1));
    let event = detector.observe_with_fingerprint(&grid_a, &grid_b, 1, fp, Some(2));
    assert!(event.is_none(), "collision should not trigger repeat");
}
