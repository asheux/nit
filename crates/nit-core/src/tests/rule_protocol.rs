//! Tests for `parse_protocol_spec` and `RuleProtocol::advance_one_gen` —
//! id/rule parsing, loop suffix, and phase progression / wrap-around.

use super::*;
use crate::gol_rules::load_rule_catalog;

#[test]
fn parse_protocol_spec_accepts_named_ids_and_inline_rules() {
    let (catalog, _) = load_rule_catalog(&[]);

    let by_id = parse_protocol_spec("conway*2>highlife*3(loop)", &catalog).unwrap();
    assert_eq!(by_id.phase_count(), 2);
    assert!(by_id.looped);
    assert_eq!(by_id.phases[0].steps, 2);
    assert_eq!(by_id.phases[1].steps, 3);

    let by_rule = parse_protocol_spec("B3/S23*1 > B36/S23*2", &catalog).unwrap();
    assert_eq!(by_rule.phase_count(), 2);
    assert!(!by_rule.looped);
}

#[test]
fn protocol_advance_loops_back_to_first_phase() {
    let (catalog, _) = load_rule_catalog(&[]);
    let mut protocol = parse_protocol_spec("conway*2>highlife*1(loop)", &catalog).unwrap();
    assert_eq!(protocol.phase_idx, 0);
    assert_eq!(protocol.step_in_phase, 0);

    protocol.advance_one_gen();
    assert_eq!((protocol.phase_idx, protocol.step_in_phase), (0, 1));

    protocol.advance_one_gen();
    assert_eq!((protocol.phase_idx, protocol.step_in_phase), (1, 0));

    // Last phase exhausted with `(loop)` suffix → wrap to first phase.
    protocol.advance_one_gen();
    assert_eq!((protocol.phase_idx, protocol.step_in_phase), (0, 0));
}
