//! Centralized tests for `parse_protocol_spec` and `RuleProtocol` —
//! phase progression, loop suffix, and canonical-string round-trip.

use super::*;
use crate::gol_rules::load_rule_catalog;

#[test]
fn protocol_parse_ids_and_rules() {
    let (catalog, _) = load_rule_catalog(&[]);
    let protocol = parse_protocol_spec("conway*2>highlife*3(loop)", &catalog).unwrap();
    assert_eq!(protocol.phase_count(), 2);
    assert!(protocol.looped);
    assert_eq!(protocol.phases[0].steps, 2);
    assert_eq!(protocol.phases[1].steps, 3);
    let protocol2 = parse_protocol_spec("B3/S23*1 > B36/S23*2", &catalog).unwrap();
    assert_eq!(protocol2.phase_count(), 2);
    assert!(!protocol2.looped);
}

#[test]
fn protocol_advance_loops() {
    let (catalog, _) = load_rule_catalog(&[]);
    let mut protocol = parse_protocol_spec("conway*2>highlife*1(loop)", &catalog).unwrap();
    assert_eq!(protocol.phase_idx, 0);
    assert_eq!(protocol.step_in_phase, 0);
    protocol.advance_one_gen();
    assert_eq!(protocol.phase_idx, 0);
    assert_eq!(protocol.step_in_phase, 1);
    protocol.advance_one_gen();
    assert_eq!(protocol.phase_idx, 1);
    assert_eq!(protocol.step_in_phase, 0);
    protocol.advance_one_gen();
    assert_eq!(protocol.phase_idx, 0);
    assert_eq!(protocol.step_in_phase, 0);
}
