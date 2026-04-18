use super::*;
use crate::step::step;

fn blinker() -> Grid {
    let mut grid = Grid::new(5, 5);
    for y in 1..=3 {
        grid.set(2, y, true);
    }
    grid
}

fn protocol_extra(protocol_hash: u64, phase_idx: u32) -> Option<AttractorExtra> {
    Some(AttractorExtra {
        protocol_hash,
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
    let g0 = blinker();
    let g1 = step(&g0, rule, edge);
    let g2 = step(&g1, rule, edge);
    let g3 = step(&g2, rule, edge);
    let g4 = step(&g3, rule, edge);

    let mut detector = AttractorDetector::new(AttractorConfig {
        policy: AutoStopPolicy::Repeat,
        ..AttractorConfig::default()
    });
    let proto_hash = 0xabcdu64;
    let proto = |phase_idx| protocol_extra(proto_hash, phase_idx);

    detector.seed_with_context(&g0, 0, rule, edge, proto(0));

    for (prev, next, gen) in [(&g0, &g1, 1u64), (&g1, &g2, 2), (&g2, &g3, 3)] {
        let phase = gen as u32;
        let event = detector.observe_with_context(prev, next, gen, rule, edge, proto(phase));
        assert!(
            event.is_none(),
            "gen {gen} (phase {phase}) is a new state, not a repeat, got {event:?}",
        );
    }

    // Phase 0 returns: blinker repeats with the original protocol context.
    let event = detector.observe_with_context(&g3, &g4, 4, rule, edge, proto(0));
    assert!(
        matches!(event, Some(AttractorEvent::Cycle { period, .. }) if period == 4),
        "phase-0 return at gen 4 should report a period-4 cycle, got {event:?}",
    );
}
