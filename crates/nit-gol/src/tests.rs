use crate::{
    analyze::evaluate_rule,
    attractor::{AttractorConfig, AttractorDetector, AttractorEvent, AutoStopPolicy},
    grid::EdgeMode,
    step::step,
    Grid, Rule,
};

// ── Fixture helpers ─────────────────────────────────────────────────

/// Build a `width`x`height` grid with the listed coordinates turned on.
fn grid_with_cells(width: usize, height: usize, live_cells: &[(usize, usize)]) -> Grid {
    let mut grid = Grid::new(width, height);
    for &(x, y) in live_cells {
        grid.set(x, y, true);
    }
    grid
}

/// Conway period-1 still life: a 2x2 block in the top-left quadrant.
fn make_block() -> Grid {
    grid_with_cells(4, 4, &[(1, 1), (1, 2), (2, 1), (2, 2)])
}

/// Conway period-2 oscillator: a vertical 3-cell line centred on (2, 2).
fn make_blinker() -> Grid {
    grid_with_cells(5, 5, &[(2, 1), (2, 2), (2, 3)])
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
            "survives mask matches digits 2 and 3",
        );
    }

    /// An empty survive section parses with a zero survive mask.
    #[test]
    fn parse_rule_empty_survive() {
        let rule = Rule::parse("B2/S").expect("parse B2/S");
        assert_eq!(rule.births_mask(), 1 << 2);
        assert_eq!(rule.survives_mask(), 0, "no survive digits = empty mask");
    }

    #[test]
    fn parse_rule_invalid_cases() {
        for candidate in ["B9/S23", "B3/S2x", "B3//S23", "B3/23", "B/S"] {
            assert!(
                Rule::parse(candidate).is_err(),
                "expected parse error for {candidate:?}",
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
                "blinker row should have cell at ({column}, 2)",
            );
        }
        let resettled = conway_step(&horizontal);
        assert_eq!(
            vertical, resettled,
            "blinker returns to original after 2 steps",
        );
    }

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
        let mut state = grid_with_cells(6, 6, &[(1, 0), (2, 1), (0, 2), (1, 2), (2, 2)]);
        for generation in 1..=4u32 {
            state = conway_step(&state);
            assert!(
                state.alive_count() > 0,
                "glider should stay alive through gen {generation}",
            );
        }
        for (x, y) in [(2, 1), (3, 2), (1, 3), (2, 3), (3, 3)] {
            assert!(
                state.get(x, y),
                "glider missing live cell at ({x}, {y}) after 4 steps",
            );
        }
    }
}

mod encoding {
    use super::*;

    #[test]
    fn grid_hash_changes() {
        let blank = Grid::new(3, 3);
        let lit = grid_with_cells(3, 3, &[(1, 1)]);
        assert_ne!(
            blank.hash(),
            lit.hash(),
            "hash must change when a cell flips",
        );
    }

    /// Every required header field and the terminator byte land in the output.
    #[test]
    fn rle_basic_sanity() {
        let canvas = grid_with_cells(3, 3, &[(1, 1)]);
        let rendered = crate::snapshot::encode_rle(&canvas, Rule::conway());
        for needle in ["x = 3", "y = 3", "rule = B3/S23"] {
            assert!(
                rendered.contains(needle),
                "RLE header must contain {needle:?}",
            );
        }
        assert!(rendered.ends_with('!'), "RLE must end with '!'");
        assert!(
            rendered.contains('o'),
            "RLE must encode the live cell as 'o'",
        );
    }

    #[test]
    fn rle_2x2_exact() {
        let canvas = grid_with_cells(2, 2, &[(0, 0), (1, 0), (1, 1)]);
        assert_eq!(render_rle(&canvas), "x = 2, y = 2, rule = B3/S23\n2o$\nbo!");
    }

    #[test]
    fn rle_5x5_exact() {
        let canvas = grid_with_cells(5, 5, &[(2, 2)]);
        assert_eq!(
            render_rle(&canvas),
            "x = 5, y = 5, rule = B3/S23\n5b$\n5b$\n2bo2b$\n5b$\n5b!",
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
    fn first_event_after_step(seed: &Grid, max_history: usize) -> Option<AttractorEvent> {
        let rule = Rule::conway();
        let edge = EdgeMode::Dead;
        let mut detector = make_repeat_detector(max_history);
        detector.seed(seed, 0, rule, edge);
        let next = conway_step(seed);
        detector.observe(seed, &next, 1, rule, edge)
    }

    /// A still-life block emits a FixedPoint event on the first observe.
    #[test]
    fn attractor_still_life_fixed_point() {
        let event = first_event_after_step(&make_block(), 128);
        assert_eq!(
            event,
            Some(AttractorEvent::FixedPoint { gen: 1 }),
            "block is a fixed point at gen 1",
        );
    }

    /// A blinker emits a Cycle event with period 2 on the second observe.
    #[test]
    fn attractor_blinker_cycle() {
        let seed = make_blinker();
        let rule = Rule::conway();
        let edge = EdgeMode::Dead;
        let mut detector = make_repeat_detector(128);
        detector.seed(&seed, 0, rule, edge);

        let step1 = conway_step(&seed);
        let first_event = detector.observe(&seed, &step1, 1, rule, edge);
        assert!(first_event.is_none(), "gen 1 is a new state, not a repeat");

        let step2 = conway_step(&step1);
        let cycle_event = detector.observe(&step1, &step2, 2, rule, edge);
        assert_eq!(
            cycle_event,
            Some(AttractorEvent::Cycle {
                gen: 2,
                first_seen: 0,
                period: 2,
                transient: 0,
            }),
            "blinker cycle detected at gen 2 with period 2",
        );
    }

    /// An empty grid is a fixed point — Conway has no spontaneous births.
    #[test]
    fn attractor_empty_grid_fixed_point() {
        let event = first_event_after_step(&Grid::new(3, 3), 64);
        assert_eq!(
            event,
            Some(AttractorEvent::FixedPoint { gen: 1 }),
            "empty grid is a fixed point at gen 1",
        );
    }

    /// Two grids with the same primary fingerprint but different secondary
    /// hashes must not be reported as a cycle (collision guard).
    #[test]
    fn attractor_hash_collision_guard() {
        let first = grid_with_cells(2, 2, &[(0, 0)]);
        let second = grid_with_cells(2, 2, &[(1, 1)]);

        let mut detector = make_repeat_detector(64);
        let fingerprint =
            AttractorDetector::test_fingerprint(0xdead_beef_dead_beef_dead_beef_dead_beef);
        detector.seed_with_fingerprint(0, fingerprint, Some(1));
        let event = detector.observe_with_fingerprint(&first, &second, 1, fingerprint, Some(2));
        assert!(
            event.is_none(),
            "differing secondary hashes must not trigger repeat",
        );
    }
}
