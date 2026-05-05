//! Cellular-automaton strategy tests.

use super::shared::record_round;
use crate::game::Action;
use crate::history::History;
use crate::strategy::{CaStrategy, Strategy};

#[test]
fn ca_rule_zero_and_255_match_notebook_behavior() {
    let mut history = History::new(0);
    record_round(&mut history, Action::Defect, Action::Cooperate);
    record_round(&mut history, Action::Defect, Action::Defect);

    let mut ca_zero = CaStrategy::new("ca0", 0, 2, 2, 1);
    let mut ca_255 = CaStrategy::new("ca255", 255, 2, 2, 1);

    assert_eq!(
        ca_zero.next_action(&History::new(0), true),
        Action::Cooperate
    );
    assert_eq!(
        ca_255.next_action(&History::new(0), true),
        Action::Cooperate
    );

    assert_eq!(ca_zero.next_action(&history, true), Action::Cooperate);
    assert_eq!(ca_255.next_action(&history, true), Action::Defect);
}

#[test]
fn ca_nontrivial_rule_matches_hand_computation() {
    // r=1 ⇒ neighborhood width 3. Rule n=85 maps each neighborhood to its rightmost bit.
    let mut history = History::new(0);
    // bits = [1,0,1,1]
    record_round(&mut history, Action::Defect, Action::Cooperate);
    record_round(&mut history, Action::Defect, Action::Defect);

    let mut ca = CaStrategy::new("ca85", 85, 2, 2, 1);
    // windows: [1,0,1] → 1, [0,1,1] → 1, final row [1,1] last cell 1 ⇒ Defect
    assert_eq!(ca.next_action(&history, true), Action::Defect);
}
