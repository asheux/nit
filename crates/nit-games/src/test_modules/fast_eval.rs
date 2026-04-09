//! Tests for the fast-eval match evaluator.

use super::{evaluate_match, FastStrategyModel};
use crate::game::{Action, PayoffMatrix};

const TEST_ROUND_COUNT: u32 = 8;

fn constant_strategy_model(label: &str, action: Action) -> FastStrategyModel {
    FastStrategyModel {
        id: label.into(),
        start: 0,
        outputs: vec![action],
        transitions: vec![0, 0],
        alphabet: 2,
    }
}

#[test]
fn outcomes_recorded_without_round_trace() {
    let cooperator_model = constant_strategy_model("always_cooperate", Action::Cooperate);
    let defector_model = constant_strategy_model("always_defect", Action::Defect);

    let prisoner_dilemma_payoff = PayoffMatrix::default_pd();

    // Run with cycle detection off, outcome recording on.
    let eval_result = evaluate_match(
        &cooperator_model,
        &defector_model,
        TEST_ROUND_COUNT,
        prisoner_dilemma_payoff,
        false,
        true,
    );

    // Every round is C-vs-D (outcome index 1).
    assert_eq!(eval_result.outcomes.as_deref(), Some("11111111"));

    // Standard PD payoffs: cooperator gets -3 per round, defector gets 0.
    assert_eq!(eval_result.a_total, -24);
    assert_eq!(eval_result.b_total, 0);
}

#[test]
fn mutual_cooperation_yields_symmetric_scores() {
    let cooperator_a = constant_strategy_model("cooperator_a", Action::Cooperate);
    let cooperator_b = constant_strategy_model("cooperator_b", Action::Cooperate);

    let prisoner_dilemma_payoff = PayoffMatrix::default_pd();

    let eval_result = evaluate_match(
        &cooperator_a,
        &cooperator_b,
        TEST_ROUND_COUNT,
        prisoner_dilemma_payoff,
        false,
        true,
    );

    // Every round is C-vs-C (outcome index 0).
    assert_eq!(eval_result.outcomes.as_deref(), Some("00000000"));

    // Both players get -1 per round under the standard PD matrix.
    assert_eq!(eval_result.a_total, eval_result.b_total);
    assert_eq!(eval_result.a_total, -8);
}

#[test]
fn mutual_defection_yields_symmetric_punishment() {
    let defector_a = constant_strategy_model("defector_a", Action::Defect);
    let defector_b = constant_strategy_model("defector_b", Action::Defect);

    let payoff = PayoffMatrix::default_pd();

    let eval_result = evaluate_match(
        &defector_a,
        &defector_b,
        TEST_ROUND_COUNT,
        payoff,
        false,
        true,
    );

    // Every round is D-vs-D (outcome index 3).
    assert_eq!(eval_result.outcomes.as_deref(), Some("33333333"));

    // Both players get -2 per round under the standard PD matrix.
    assert_eq!(eval_result.a_total, eval_result.b_total);
    assert_eq!(eval_result.a_total, -16);
}
