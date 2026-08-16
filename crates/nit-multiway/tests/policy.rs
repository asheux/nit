use nit_core::GenomeTier;
use nit_multiway::policy::{frontier_admission, is_solution, Budget, Mood, SearchPolicy};
use nit_multiway::Value;

#[test]
fn default_policy_is_balanced_with_sane_caps() {
    let policy = SearchPolicy::default();

    assert_eq!(policy.mood, Mood::Balanced);
    assert_eq!(policy.k, 3);
    assert!(policy.budget.max_nodes > 0);
    assert!(policy.budget.max_turns > 0);
    assert_eq!(policy.budget.max_tokens, None);
}

#[test]
fn policy_fields_are_publicly_constructible() {
    let policy = SearchPolicy {
        mood: Mood::Exploit,
        k: 5,
        budget: Budget {
            max_nodes: 10,
            max_turns: 4,
            max_tokens: Some(1_000),
        },
    };

    assert!(matches!(policy.mood, Mood::Exploit));
    assert_eq!(policy.k, 5);
    assert_eq!(policy.budget.max_tokens, Some(1_000));
}

#[test]
fn frontier_admission_scales_width_to_mood() {
    // (mood, viable children, admitted) — Exploit follows one line, Explore keeps
    // all, Balanced takes the rounded-up half; the remainder become Held. A lone
    // viable child is never stranded, and an empty expansion admits nothing.
    let cases = [
        (Mood::Exploit, 4, 1),
        (Mood::Explore, 4, 4),
        (Mood::Balanced, 4, 2),
        (Mood::Balanced, 5, 3),
        (Mood::Balanced, 1, 1),
        (Mood::Exploit, 0, 0),
    ];
    for (mood, viable, admitted) in cases {
        assert_eq!(
            frontier_admission(mood, viable),
            admitted,
            "{mood:?} over {viable} viable children"
        );
    }
}

#[test]
fn only_ungated_top_tier_nodes_are_solutions() {
    let solved = Value {
        gated: false,
        score: 0.9,
        tier: GenomeTier::Replicator,
    };
    assert!(is_solution(&solved));

    // A gate failure disqualifies a node even at the top tier...
    assert!(!is_solution(&Value {
        gated: true,
        ..solved
    }));
    // ...and anything below Replicator runs to budget instead of stopping early.
    assert!(!is_solution(&Value {
        tier: GenomeTier::Methuselah,
        ..solved
    }));
}
