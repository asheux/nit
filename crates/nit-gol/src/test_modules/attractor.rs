use super::*;
use crate::step::step;

/// Protocol-aware detection only matches when the protocol hash and
/// phase index align — not just the grid state.
#[test]
fn repeat_requires_matching_protocol_phase() {
    let rule = Rule::conway();
    let edge = EdgeMode::Dead;
    let mut grid = Grid::new(5, 5);
    grid.set(2, 1, true);
    grid.set(2, 2, true);
    grid.set(2, 3, true);

    let g0 = grid.clone();
    let g1 = step(&g0, rule, edge);
    let g2 = step(&g1, rule, edge);
    let g3 = step(&g2, rule, edge);
    let g4 = step(&g3, rule, edge);

    let mut detector = AttractorDetector::new(AttractorConfig {
        policy: AutoStopPolicy::Repeat,
        ..AttractorConfig::default()
    });
    let proto_hash = 0xabcdu64;
    let proto = |phase_idx| {
        Some(AttractorExtra {
            protocol_hash: proto_hash,
            phase_idx,
            step_in_phase: 0,
        })
    };

    detector.seed_with_context(&g0, 0, rule, edge, proto(0));

    // Phases 1..=3 introduce new states; no repeat yet.
    assert!(detector
        .observe_with_context(&g0, &g1, 1, rule, edge, proto(1))
        .is_none());
    assert!(detector
        .observe_with_context(&g1, &g2, 2, rule, edge, proto(2))
        .is_none());
    assert!(detector
        .observe_with_context(&g2, &g3, 3, rule, edge, proto(3))
        .is_none());

    // Phase 0 returns: blinker repeats with the original protocol context.
    let event = detector.observe_with_context(&g3, &g4, 4, rule, edge, proto(0));
    assert!(matches!(
        event,
        Some(AttractorEvent::Cycle { period, .. }) if period == 4
    ));
}
