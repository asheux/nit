use super::*;
use crate::step::step;

const PROTOCOL_HASH: u64 = 0xabcd;

fn make_blinker() -> Grid {
    let mut grid = Grid::new(5, 5);
    for y in 1..=3 {
        grid.set(2, y, true);
    }
    grid
}

fn phase_context(phase_idx: u32) -> Option<AttractorExtra> {
    Some(AttractorExtra {
        protocol_hash: PROTOCOL_HASH,
        phase_idx,
        step_in_phase: 0,
    })
}

/// Protocol-aware detection only matches when the protocol hash and
/// phase index align — not just the grid state.
#[test]
fn repeat_requires_matching_protocol_phase() {
    let rule = Rule::conway();
    let edge = EdgeMode::Dead;
    let g0 = make_blinker();
    let g1 = step(&g0, rule, edge);
    let g2 = step(&g1, rule, edge);
    let g3 = step(&g2, rule, edge);
    let g4 = step(&g3, rule, edge);

    let mut detector = AttractorDetector::new(AttractorConfig {
        policy: AutoStopPolicy::Repeat,
        ..AttractorConfig::default()
    });
    detector.seed_with_context(&g0, 0, rule, edge, phase_context(0));

    // Generations 1..=3 visit distinct (state, phase) pairs — none should report a repeat.
    let ascending_phase_steps: [(&Grid, &Grid, u64, u32); 3] =
        [(&g0, &g1, 1, 1), (&g1, &g2, 2, 2), (&g2, &g3, 3, 3)];
    for (prev, next, gen, phase) in ascending_phase_steps {
        let event =
            detector.observe_with_context(prev, next, gen, rule, edge, phase_context(phase));
        assert!(
            event.is_none(),
            "gen {gen} (phase {phase}) is a new state, not a repeat, got {event:?}",
        );
    }

    // Phase 0 returns: blinker repeats with the original protocol context.
    let event = detector.observe_with_context(&g3, &g4, 4, rule, edge, phase_context(0));
    assert!(
        matches!(event, Some(AttractorEvent::Cycle { period, .. }) if period == 4),
        "phase-0 return at gen 4 should report a period-4 cycle, got {event:?}",
    );
}
