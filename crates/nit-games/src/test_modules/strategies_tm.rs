//! Turing-machine strategy tests + the timeout-payoff tie-in.

use super::shared::record_round;
use crate::game::{payoffs_with_timeouts, Action, PayoffMatrix};
use crate::history::History;
use crate::strategy::{
    decode_tm_rule_code_wolfram, history_to_input_u64, OneSidedTmStrategy, Strategy, TmMove,
    TmTransition,
};

#[test]
fn tm_always_move_right_write_zero_cooperates_and_halts() {
    let transitions = vec![
        TmTransition {
            write: 0,
            move_dir: TmMove::Right,
            next: 1,
        },
        TmTransition {
            write: 0,
            move_dir: TmMove::Right,
            next: 1,
        },
    ];
    let mut tm = OneSidedTmStrategy::new("tm", 2, 1, 0, 4, transitions);

    let mut history = History::new(0);
    for _ in 0..3 {
        let action = tm.next_action(&history, true);
        assert_eq!(action, Action::Cooperate);
        assert!(tm.last_halted());
        record_round(&mut history, action, Action::Defect);
    }
}

#[test]
fn tm_rule_code_zero_cooperates_on_first_round_then_times_out() {
    let (transitions, _) = decode_tm_rule_code_wolfram(0, 1, 2);
    let mut tm = OneSidedTmStrategy::new("tm_zero", 2, 1, 0, 4, transitions);
    let mut history = History::new(0);
    let action = tm.next_action(&history, true);
    assert_eq!(action, Action::Cooperate);
    assert!(tm.last_halted());

    record_round(&mut history, Action::Cooperate, Action::Defect);
    let action = tm.next_action(&history, true);
    assert_eq!(action, Action::Defect);
    assert!(!tm.last_halted());
}

#[test]
fn history_to_input_uses_flattened_pairs_binary_order() {
    let mut history = History::new(0);
    record_round(&mut history, Action::Cooperate, Action::Defect); // bits [0,1]
    assert_eq!(history_to_input_u64(&history), Some(1));
}

#[test]
fn timeout_scoring_matches_notebook_min_max_logic() {
    let payoff = PayoffMatrix::default_pd(); // min=-3, max=0
    let both_halted = payoffs_with_timeouts(payoff, Action::Cooperate, Action::Defect, true, true);
    assert_eq!(
        both_halted,
        payoff.payoffs(Action::Cooperate, Action::Defect)
    );

    let a_timeout = payoffs_with_timeouts(payoff, Action::Cooperate, Action::Defect, false, true);
    assert_eq!(a_timeout, (-3, 0));

    let b_timeout = payoffs_with_timeouts(payoff, Action::Cooperate, Action::Defect, true, false);
    assert_eq!(b_timeout, (0, -3));

    let both_timeout =
        payoffs_with_timeouts(payoff, Action::Cooperate, Action::Defect, false, false);
    assert_eq!(both_timeout, (-3, -3));
}
