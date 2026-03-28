use super::{evaluate_match, FastStrategyModel};
use crate::game::{Action, PayoffMatrix};

#[test]
fn evaluate_match_can_record_outcomes_without_round_trace() {
    let always_cooperate = FastStrategyModel {
        id: "all_c".into(),
        start: 0,
        outputs: vec![Action::Cooperate],
        transitions: vec![0, 0],
        alphabet: 2,
    };
    let always_defect = FastStrategyModel {
        id: "all_d".into(),
        start: 0,
        outputs: vec![Action::Defect],
        transitions: vec![0, 0],
        alphabet: 2,
    };

    let eval = evaluate_match(
        &always_cooperate,
        &always_defect,
        8,
        PayoffMatrix::default_pd(),
        false,
        true,
    );

    assert_eq!(eval.outcomes.as_deref(), Some("11111111"));
    assert_eq!(eval.a_total, -24);
    assert_eq!(eval.b_total, 0);
}
