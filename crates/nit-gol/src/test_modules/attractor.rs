use super::*;
use crate::step::step;

/// Verify that protocol-aware detection only matches when all extra
/// fields (protocol hash, phase index) align — not just the grid state.
#[test]
fn repeat_requires_matching_protocol_phase() {
    let rule = Rule::conway();
    let mut grid = Grid::new(5, 5);
    grid.set(2, 1, true);
    grid.set(2, 2, true);
    grid.set(2, 3, true);

    let g0 = grid.clone();
    let g1 = step(&g0, rule, EdgeMode::Dead);
    let g2 = step(&g1, rule, EdgeMode::Dead);
    let g3 = step(&g2, rule, EdgeMode::Dead);
    let g4 = step(&g3, rule, EdgeMode::Dead);

    let mut detector = AttractorDetector::new(AttractorConfig {
        policy: AutoStopPolicy::Repeat,
        ..AttractorConfig::default()
    });
    let proto_hash = 0xabcdu64;
    detector.seed_with_context(
        &g0,
        0,
        rule,
        EdgeMode::Dead,
        Some(AttractorExtra {
            protocol_hash: proto_hash,
            phase_idx: 0,
            step_in_phase: 0,
        }),
    );

    // Phase 1 — new state, no repeat expected.
    let event1 = detector.observe_with_context(
        &g0,
        &g1,
        1,
        rule,
        EdgeMode::Dead,
        Some(AttractorExtra {
            protocol_hash: proto_hash,
            phase_idx: 1,
            step_in_phase: 0,
        }),
    );
    assert!(event1.is_none());

    // Phase 2 — still accumulating.
    let event2 = detector.observe_with_context(
        &g1,
        &g2,
        2,
        rule,
        EdgeMode::Dead,
        Some(AttractorExtra {
            protocol_hash: proto_hash,
            phase_idx: 2,
            step_in_phase: 0,
        }),
    );
    assert!(event2.is_none());

    // Phase 3 — one more unique state.
    let event3 = detector.observe_with_context(
        &g2,
        &g3,
        3,
        rule,
        EdgeMode::Dead,
        Some(AttractorExtra {
            protocol_hash: proto_hash,
            phase_idx: 3,
            step_in_phase: 0,
        }),
    );
    assert!(event3.is_none());

    // Phase 0 again — the blinker repeats with the same protocol context.
    let event4 = detector.observe_with_context(
        &g3,
        &g4,
        4,
        rule,
        EdgeMode::Dead,
        Some(AttractorExtra {
            protocol_hash: proto_hash,
            phase_idx: 0,
            step_in_phase: 0,
        }),
    );
    assert!(matches!(
        event4,
        Some(AttractorEvent::Cycle { period, .. }) if period == 4
    ));
}
