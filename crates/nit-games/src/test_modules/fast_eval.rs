//! Tests for the fast-eval match evaluator.

use crate::config::{StrategySpec, StrategySpecKind};
use crate::fast_eval::{evaluate_match, FastEvalResult, FastStrategyModel};
use crate::game::{Action, PayoffMatrix};
use crate::strategy::InputMode;

const TEST_ROUND_COUNT: u32 = 8;

fn constant_strategy_model(label: &str, action: Action) -> FastStrategyModel {
    let spec = StrategySpec {
        id: label.into(),
        name: None,
        kind: StrategySpecKind::Fsm {
            num_states: 1,
            start_state: 0,
            outputs: vec![action],
            input_mode: Some(InputMode::OpponentLastAction),
            transitions: vec![vec![0, 0]],
            index: None,
        },
    };
    FastStrategyModel::from_spec(&spec).expect("constant fsm spec is valid")
}

fn run_match(a: &FastStrategyModel, b: &FastStrategyModel) -> FastEvalResult {
    evaluate_match(
        a,
        b,
        TEST_ROUND_COUNT,
        PayoffMatrix::default_pd(),
        false,
        true,
    )
}

#[test]
fn outcomes_recorded_without_round_trace() {
    let cooperator = constant_strategy_model("always_cooperate", Action::Cooperate);
    let defector = constant_strategy_model("always_defect", Action::Defect);

    let result = run_match(&cooperator, &defector);

    // Every round is C-vs-D (outcome index 1).
    assert_eq!(result.outcomes.as_deref(), Some("11111111"));
    // Standard PD payoffs: cooperator gets -3 per round, defector gets 0.
    assert_eq!(result.a_total, -24);
    assert_eq!(result.b_total, 0);
}

#[test]
fn mutual_cooperation_yields_symmetric_scores() {
    let a = constant_strategy_model("cooperator_a", Action::Cooperate);
    let b = constant_strategy_model("cooperator_b", Action::Cooperate);

    let result = run_match(&a, &b);

    // Every round is C-vs-C (outcome index 0).
    assert_eq!(result.outcomes.as_deref(), Some("00000000"));
    // Both players get -1 per round under the standard PD matrix.
    assert_eq!(result.a_total, result.b_total);
    assert_eq!(result.a_total, -8);
}

#[test]
fn mutual_defection_yields_symmetric_punishment() {
    let a = constant_strategy_model("defector_a", Action::Defect);
    let b = constant_strategy_model("defector_b", Action::Defect);

    let result = run_match(&a, &b);

    // Every round is D-vs-D (outcome index 3).
    assert_eq!(result.outcomes.as_deref(), Some("33333333"));
    // Both players get -2 per round under the standard PD matrix.
    assert_eq!(result.a_total, result.b_total);
    assert_eq!(result.a_total, -16);
}
