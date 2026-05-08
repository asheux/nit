//! FSM strategy tests: notebook-index decoding, behaviour grouping, and the
//! canonical-rep tables that downstream code reads back as references.

use super::shared::{
    notebook_buggy_fsm_to_index_from_rules, notebook_buggy_initial_action,
    notebook_buggy_state_outputs, notebook_rules_from_outputs_and_transitions, record_round,
};
use crate::config::FsmGroupingMode;
use crate::game::Action;
use crate::history::History;
use crate::strategy::{decode_fsm_notebook_index, FsmStrategy, InputMode, Strategy};
use crate::{
    canonical_fsm_indices, group_canonical_fsm_indices_by_behavior,
    group_canonical_fsm_indices_by_behavior_with_mode, unique_fsm_behavior_representatives,
    unique_fsm_behavior_representatives_with_mode,
};

fn make_fsm(label: &str, outputs: Vec<Action>, transitions: Vec<Vec<usize>>) -> FsmStrategy {
    FsmStrategy::new(
        label,
        0,
        outputs,
        InputMode::OpponentLastAction,
        transitions,
    )
}

#[test]
fn fsm_notebook_index_s1_k2_all_d_and_all_c() {
    let (outputs_0, transitions_0) = decode_fsm_notebook_index(0, 1, 2).expect("decode idx 0");
    let (outputs_1, transitions_1) = decode_fsm_notebook_index(1, 1, 2).expect("decode idx 1");

    assert_eq!(outputs_0, vec![Action::Defect]);
    assert_eq!(outputs_1, vec![Action::Cooperate]);
    assert_eq!(transitions_0, vec![vec![0, 0]]);
    assert_eq!(transitions_1, vec![vec![0, 0]]);

    let mut all_d = make_fsm("all_d", outputs_0, transitions_0);
    let mut all_c = make_fsm("all_c", outputs_1, transitions_1);

    let mut history = History::new(0);
    assert_eq!(all_d.next_action(&history, true), Action::Defect);
    assert_eq!(all_c.next_action(&history, true), Action::Cooperate);

    record_round(&mut history, Action::Cooperate, Action::Defect);
    assert_eq!(all_d.next_action(&history, true), Action::Defect);
    assert_eq!(all_c.next_action(&history, true), Action::Cooperate);
}

#[test]
fn notebook_fsmstrategyfunction_drops_unreferenced_start_state_output() {
    let outputs = vec![Action::Cooperate, Action::Defect];
    let transitions = vec![vec![1, 1], vec![1, 1]];

    assert_eq!(outputs[0], Action::Cooperate);
    assert_eq!(
        notebook_buggy_state_outputs(&outputs, &transitions),
        vec![None, Some(Action::Defect)]
    );
    assert_eq!(
        notebook_buggy_initial_action(&outputs, &transitions),
        Action::Defect
    );
}

#[test]
fn notebook_fsm_to_index_is_not_inverse_when_a_state_has_no_incoming_edges() {
    let transitions = vec![vec![1, 1], vec![1, 1]];
    let outputs_a = vec![Action::Cooperate, Action::Defect];
    let outputs_b = vec![Action::Defect, Action::Defect];

    let rules_a = notebook_rules_from_outputs_and_transitions(&outputs_a, &transitions);
    let rules_b = notebook_rules_from_outputs_and_transitions(&outputs_b, &transitions);

    assert_eq!(rules_a, rules_b);
    assert_eq!(
        notebook_buggy_fsm_to_index_from_rules(&rules_a, 2, 2),
        notebook_buggy_fsm_to_index_from_rules(&rules_b, 2, 2)
    );
}

#[test]
fn user_reported_code_02_top_strategies_include_buggy_start_output_cases() {
    let affected = unique_fsm_behavior_representatives(3, 2)
        .expect("fsm behavior representatives")
        .into_iter()
        .filter(|&index| {
            let Ok((outputs, transitions)) = decode_fsm_notebook_index(index, 3, 2) else {
                return false;
            };
            outputs.first() == Some(&Action::Cooperate)
                && notebook_buggy_initial_action(&outputs, &transitions) != outputs[0]
        })
        .collect::<Vec<_>>();

    assert_eq!(affected.len(), 36);
    for expected in [3563, 3234, 3882] {
        assert!(
            affected.contains(&expected),
            "missing expected idx {expected}"
        );
    }
}

#[test]
fn fsm_uses_opponent_last_action_like_tft() {
    let outputs = vec![Action::Cooperate, Action::Defect];
    let transitions = vec![vec![0, 1], vec![0, 1]];

    let mut a_fsm = make_fsm("tft_a", outputs.clone(), transitions.clone());
    let mut b_fsm = make_fsm("tft_b", outputs, transitions);

    let mut history = History::new(0);
    assert_eq!(a_fsm.next_action(&history, true), Action::Cooperate);
    assert_eq!(b_fsm.next_action(&history, false), Action::Cooperate);

    record_round(&mut history, Action::Cooperate, Action::Defect);
    assert_eq!(a_fsm.next_action(&history, true), Action::Defect);
    assert_eq!(b_fsm.next_action(&history, false), Action::Cooperate);

    record_round(&mut history, Action::Defect, Action::Cooperate);
    assert_eq!(a_fsm.next_action(&history, true), Action::Cooperate);
    assert_eq!(b_fsm.next_action(&history, false), Action::Defect);
}

#[test]
fn fsm_canonical_indices_s1_k2_match_allc_alld() {
    let canonical = canonical_fsm_indices(1, 2).expect("canonical indices");
    assert_eq!(canonical, vec![0, 1]);

    let groups = group_canonical_fsm_indices_by_behavior(1, 2)
        .expect("grouped canonical indices by behavior");
    assert_eq!(groups, vec![vec![0], vec![1]]);

    let reps = unique_fsm_behavior_representatives(1, 2).expect("behavior representatives");
    assert_eq!(reps, vec![0, 1]);
}

#[test]
fn fsm_unique_behavior_reps_s2_k2_match_notebook_reference_set_by_default() {
    let canonical = unique_fsm_behavior_representatives(2, 2).expect("behavior representatives");
    let expected = vec![
        0, 1, 18, 19, 22, 23, 26, 27, 30, 31, 34, 35, 38, 39, 46, 47, 50, 51, 54, 55, 58, 59,
    ];
    assert_eq!(canonical, expected);
}

#[test]
fn fsm_unique_behavior_reps_s3_k2_match_notebook_reference_count_by_default() {
    let canonical = unique_fsm_behavior_representatives(3, 2).expect("behavior representatives");
    assert_eq!(canonical.len(), 956);
}

#[test]
fn fsm_unique_behavior_reps_s3_k2_match_saved_code_02_reference_prefix_and_suffix() {
    let canonical = unique_fsm_behavior_representatives(3, 2).expect("behavior representatives");
    let expected_prefix = vec![
        0, 1, 651, 653, 723, 725, 794, 795, 796, 797, 798, 799, 802, 807, 810, 811, 814, 815, 818,
        819, 820, 821, 822, 823, 826, 827, 828, 829, 830, 831, 834, 835, 836, 837, 838, 839, 842,
        843, 844, 845,
    ];
    let expected_suffix = vec![
        3836, 3837, 3838, 3839, 3842, 3844, 3845, 3847, 3850, 3851, 3866, 3867, 3868, 3869, 3870,
        3871, 3874, 3875, 3882, 3883,
    ];
    assert_eq!(
        &canonical[..expected_prefix.len()],
        expected_prefix.as_slice()
    );
    assert_eq!(
        &canonical[canonical.len() - expected_suffix.len()..],
        expected_suffix.as_slice()
    );
}

#[test]
fn fsm_exact_grouping_mode_matches_exact_minimization_reference() {
    let reps_22 = unique_fsm_behavior_representatives_with_mode(2, 2, FsmGroupingMode::Moorem)
        .expect("exact behavior representatives");
    let expected_22 = vec![
        0, 1, 18, 19, 22, 23, 26, 27, 30, 31, 34, 35, 38, 39, 42, 43, 46, 47, 50, 51, 54, 55, 58,
        59, 62, 63,
    ];
    assert_eq!(reps_22, expected_22);

    let reps_32 = unique_fsm_behavior_representatives_with_mode(3, 2, FsmGroupingMode::Moorem)
        .expect("exact behavior representatives");
    assert_eq!(reps_32.len(), 1054);
}

#[test]
fn fsm_canonical_groupings_are_consistent_for_multiple_s_k() {
    for (states, actions) in [(2usize, 2usize), (3, 2)] {
        let canonical =
            canonical_fsm_indices(states, actions).expect("canonical indices for (s, k)");
        assert!(!canonical.is_empty());

        let mut sorted = canonical.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(canonical, sorted);

        for mode in [FsmGroupingMode::Wnbm, FsmGroupingMode::Moorem] {
            let groups = group_canonical_fsm_indices_by_behavior_with_mode(states, actions, mode)
                .expect("grouped canonical indices by behavior");
            assert!(!groups.is_empty());
            assert!(groups.iter().all(|group| !group.is_empty()));

            let mut flattened = groups.iter().flatten().copied().collect::<Vec<_>>();
            flattened.sort_unstable();
            assert_eq!(flattened, canonical);

            let reps = unique_fsm_behavior_representatives_with_mode(states, actions, mode)
                .expect("behavior representatives");
            assert_eq!(reps.len(), groups.len());
            assert!(groups
                .iter()
                .zip(reps.iter())
                .all(|(group, rep)| group.first() == Some(rep)));
        }
    }
}
