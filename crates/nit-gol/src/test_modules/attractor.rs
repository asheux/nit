use super::*;
use crate::step::step;
use crate::{EdgeMode, Grid, Rule};

const PROTOCOL_HASH: u64 = 0xabcd;

/// Conway period-2 oscillator (blinker) centred on a 5x5 grid.
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

fn repeat_detector() -> AttractorDetector {
    AttractorDetector::new(AttractorConfig {
        policy: AutoStopPolicy::Repeat,
        ..AttractorConfig::default()
    })
}

/// Evolve `seed` forward `steps` generations under Conway's rules and
/// return the chain `[seed, step¹, step², …]` of length `steps + 1`.
fn evolve_chain(seed: Grid, steps: usize, rule: Rule, edge: EdgeMode) -> Vec<Grid> {
    let mut chain = Vec::with_capacity(steps + 1);
    chain.push(seed);
    for _ in 0..steps {
        let prev = chain.last().expect("chain seeded above");
        chain.push(step(prev, rule, edge));
    }
    chain
}

/// Protocol-aware detection only matches when grid state AND protocol
/// phase align — identical grids observed in distinct phases must not
/// be reported as a cycle.
#[test]
fn repeat_requires_matching_protocol_phase() {
    let rule = Rule::conway();
    let edge = EdgeMode::Dead;
    let chain = evolve_chain(make_blinker(), 4, rule, edge);

    let mut detector = repeat_detector();
    detector.seed_with_context(&chain[0], 0, rule, edge, phase_context(0));

    // Generations 1..=3 each visit a fresh (state, phase) pair.
    for (pair, generation) in chain.windows(2).zip(1u64..).take(3) {
        let phase = generation as u32;
        let event = detector.observe_with_context(
            &pair[0],
            &pair[1],
            generation,
            rule,
            edge,
            phase_context(phase),
        );
        assert!(
            event.is_none(),
            "gen {generation} (phase {phase}) is a new (state, phase) pair, got {event:?}",
        );
    }

    // Phase 0 returns: the blinker repeats with the original protocol context.
    let event =
        detector.observe_with_context(&chain[3], &chain[4], 4, rule, edge, phase_context(0));
    assert!(
        matches!(event, Some(AttractorEvent::Cycle { period, .. }) if period == 4),
        "phase-0 return at gen 4 should report a period-4 cycle, got {event:?}",
    );
}
