//! Tests for the fast-eval (cycle-detecting) match evaluator.
//!
//! These tests exercise [`evaluate_match`] directly, verifying that the
//! optimised FSM-to-FSM evaluation path produces correct scores and
//! outcome strings without requiring the full tournament kernel.

use super::{evaluate_match, FastStrategyModel};
use crate::game::{Action, PayoffMatrix};

// ── Helpers ───────────────────────────────────────────────────────────────

/// Number of rounds used in the standard test scenarios.
const TEST_ROUND_COUNT: u32 = 8;

/// Build a single-state FSM that unconditionally plays the given action
/// every round (Always-Cooperate or Always-Defect).
fn constant_strategy_model(label: &str, action: Action) -> FastStrategyModel {
    FastStrategyModel {
        id: label.into(),
        start: 0,
        outputs: vec![action],
        transitions: vec![0, 0],
        alphabet: 2,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

/// Always-C vs Always-D over 8 rounds: every round is outcome index 1,
/// yielding scores of -24 and 0 under the standard PD matrix.
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

/// Always-C vs Always-C: outcome index 0 every round, both get -1/round.
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
