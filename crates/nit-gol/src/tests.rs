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
    for (x, y) in [(1, 1), (1, 2), (2, 1), (2, 2)] {
        grid.set(x, y, true);
    }
    grid
}

/// Conway period-2 oscillator: a vertical 3-cell line centered on (2, 2).
fn make_blinker() -> Grid {
    let mut grid = Grid::new(5, 5);
    for y in 1..=3 {
        grid.set(2, y, true);
    }
    grid
}

fn make_repeat_detector(max_history: usize) -> AttractorDetector {
    AttractorDetector::new(AttractorConfig {
        policy: AutoStopPolicy::Repeat,
        max_history,
        confirm_on_repeat: true,
    })
}

/// Advance one generation under Conway's rules with a dead border.
fn conway_step(grid: &Grid) -> Grid {
    step(grid, Rule::conway(), EdgeMode::Dead)
}

mod parsing {
    use super::*;

    /// Parse B36/S23 and verify the canonical round-trip.
    #[test]
    fn parse_rule_roundtrip() {
        let rule = Rule::parse("B36/S23").expect("parse B36/S23");
        assert_eq!(rule.to_string(), "B36/S23", "canonical round-trip");
    }

    /// Individual bits of the birth and survival masks line up with the digits.
    #[test]
    fn parse_rule_masks() {
        let rule = Rule::parse("B3/S23").expect("parse B3/S23");
        assert_eq!(rule.births_mask(), 1 << 3, "births mask matches digit 3");
        assert_eq!(
            rule.survives_mask(),
            (1 << 2) | (1 << 3),
            "survives mask matches digits 2 and 3"
        );
    }

    /// An empty survive section parses with a zero survive mask.
    #[test]
    fn parse_rule_empty_survive() {
        let rule = Rule::parse("B2/S").expect("parse B2/S");
        assert_eq!(rule.births_mask(), 1 << 2);
        assert_eq!(rule.survives_mask(), 0, "no survive digits = empty mask");
    }

    /// Malformed rule strings all fail to parse.
    #[test]
    fn parse_rule_invalid_cases() {
        let rejects = ["B9/S23", "B3/S2x", "B3//S23", "B3/23", "B/S"];
        for candidate in rejects {
            assert!(
                Rule::parse(candidate).is_err(),
                "expected parse error for {candidate:?}"
            );
        }
    }
}

mod evolution {
    use super::*;

    /// A 2x2 block is a period-1 still life under Conway's rules.
    #[test]
    fn block_still_life() {
        let before = make_block();
        let after = conway_step(&before);
        assert_eq!(before, after, "block should be unchanged after one step");
    }

    /// A 3-cell line oscillates with period 2 (the blinker).
    #[test]
    fn blinker_oscillator() {
        let vertical = make_blinker();
        let horizontal = conway_step(&vertical);
        for column in 1..=3 {
            assert!(
                horizontal.get(column, 2),
                "blinker row should have cell at ({column}, 2)"
            );
        }
        let resettled = conway_step(&horizontal);
        assert_eq!(
            vertical, resettled,
            "blinker returns to original after 2 steps"
        );
    }

    /// The rule evaluator should detect the blinker's period-2 cycle.
    #[test]
    fn evaluate_rule_detects_period() {
        let seed = make_blinker();
        let report = evaluate_rule(&seed, Rule::conway(), EdgeMode::Dead, 10);
        assert_eq!(report.period, Some(2), "blinker period");
        assert!(report.score > 0.0, "non-zero score for valid oscillator");
    }

    /// A glider translates one cell down-right every 4 generations.
    #[test]
    fn glider_moves_down_right() {
        let mut state = Grid::new(6, 6);
        for (col, row) in [(1, 0), (2, 1), (0, 2), (1, 2), (2, 2)] {
            state.set(col, row, true);
        }
        for generation in 1..=4u32 {
            state = conway_step(&state);
            assert!(
                state.alive_count() > 0,
                "glider should stay alive through gen {generation}"
            );
        }
        for (col, row) in [(2, 1), (3, 2), (1, 3), (2, 3), (3, 3)] {
            assert!(
                state.get(col, row),
                "glider missing live cell at ({col}, {row}) after 4 steps"
            );
        }
    }
}

mod encoding {
    use super::*;

    /// Flipping a cell changes the grid hash.
    #[test]
    fn grid_hash_changes() {
        let mut canvas = Grid::new(3, 3);
        let blank_hash = canvas.hash();
        canvas.set(1, 1, true);
        let lit_hash = canvas.hash();
        assert_ne!(blank_hash, lit_hash, "hash must change when a cell flips");
    }

    /// Header fields present and terminator correct.
    #[test]
    fn rle_basic_sanity() {
        let mut canvas = Grid::new(3, 3);
        canvas.set(1, 1, true);
        let rendered = crate::snapshot::encode_rle(&canvas, Rule::conway());
        for needle in ["x = 3", "y = 3", "rule = B3/S23"] {
            assert!(
                rendered.contains(needle),
                "RLE header must contain {needle:?}"
            );
        }
        assert!(rendered.ends_with('!'), "RLE must end with '!'");
        assert!(
            rendered.contains('o'),
            "RLE must encode the live cell as 'o'"
        );
    }

    /// Exact byte-level RLE output for a 2x2 grid.
    #[test]
    fn rle_2x2_exact() {
        let mut canvas = Grid::new(2, 2);
        for (x, y) in [(0, 0), (1, 0), (1, 1)] {
            canvas.set(x, y, true);
        }
        assert_eq!(render_rle(&canvas), "x = 2, y = 2, rule = B3/S23\n2o$\nbo!");
    }

    /// Exact byte-level RLE output for a sparse 5x5 grid.
    #[test]
    fn rle_5x5_exact() {
        let mut canvas = Grid::new(5, 5);
        canvas.set(2, 2, true);
        assert_eq!(
            render_rle(&canvas),
            "x = 5, y = 5, rule = B3/S23\n5b$\n5b$\n2bo2b$\n5b$\n5b!"
        );
    }

    fn render_rle(grid: &Grid) -> String {
        let mut buffer = Vec::new();
        crate::snapshot::write_rle(&mut buffer, grid, Rule::conway())
            .expect("write_rle should succeed on in-memory buffer");
        String::from_utf8(buffer).expect("RLE output must be valid UTF-8")
    }
}

mod attractor_detection {
    use super::*;

    /// Seed, advance one generation, and return the detector event.
    fn run_single_step(grid: &Grid, max_history: usize) -> Option<AttractorEvent> {
        let rule = Rule::conway();
        let edge = EdgeMode::Dead;
        let mut detector = make_repeat_detector(max_history);
        detector.seed(grid, 0, rule, edge);
        let next = conway_step(grid);
        detector.observe(grid, &next, 1, rule, edge)
    }

    /// A still-life block emits a FixedPoint event on the first observe.
    #[test]
    fn attractor_still_life_fixed_point() {
        let event = run_single_step(&make_block(), 128);
        assert_eq!(
            event,
            Some(AttractorEvent::FixedPoint { gen: 1 }),
            "block is a fixed point at gen 1"
        );
    }

    /// A blinker emits a Cycle event with period 2.
    #[test]
    fn attractor_blinker_cycle() {
        let seed = make_blinker();
        let rule = Rule::conway();
        let edge = EdgeMode::Dead;
        let mut detector = make_repeat_detector(128);
        detector.seed(&seed, 0, rule, edge);

        let g1 = conway_step(&seed);
        let first = detector.observe(&seed, &g1, 1, rule, edge);
        assert!(first.is_none(), "gen 1 is a new state, not a repeat");

        let g2 = conway_step(&g1);
        let repeat = detector.observe(&g1, &g2, 2, rule, edge);
        assert_eq!(
            repeat,
            Some(AttractorEvent::Cycle {
                gen: 2,
                first_seen: 0,
                period: 2,
                transient: 0,
            }),
            "blinker cycle detected at gen 2 with period 2"
        );
    }

    /// An empty grid is a fixed point — Conway has no spontaneous births.
    #[test]
    fn attractor_empty_grid_fixed_point() {
        let event = run_single_step(&Grid::new(3, 3), 64);
        assert_eq!(
            event,
            Some(AttractorEvent::FixedPoint { gen: 1 }),
            "empty grid is a fixed point at gen 1"
        );
    }

    /// Two grids with the same fingerprint but different secondary hashes
    /// are not treated as a cycle (collision guard).
    #[test]
    fn attractor_hash_collision_guard() {
        let mut first = Grid::new(2, 2);
        first.set(0, 0, true);
        let mut second = Grid::new(2, 2);
        second.set(1, 1, true);

        let mut detector = make_repeat_detector(64);
        let fp = AttractorDetector::test_fingerprint(0xdead_beef_dead_beef_dead_beef_dead_beef);
        detector.seed_with_fingerprint(0, fp, Some(1));
        let event = detector.observe_with_fingerprint(&first, &second, 1, fp, Some(2));
        assert!(
            event.is_none(),
            "differing secondary hashes must not trigger repeat"
        );
    }
}
