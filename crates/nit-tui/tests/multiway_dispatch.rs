//! Mapping contract for `@all mode=multiway` (`multiway/dispatch.rs`): the broadcast
//! fan-out becomes the fork width `k`, clamped to the FD-bounded swarm ceiling, and
//! the body becomes the search task. Expectations are derived from the same
//! `clamp_fork_width` + `effective_max_swarm_size` the runtime uses, so the
//! assertions hold on any host regardless of its `ulimit -n`.

use nit_tui::multiway::dispatch::all_multiway_policy;
use nit_tui::multiway::runtime::clamp_fork_width;
use nit_tui::multiway::{Budget, Mood, SearchPolicy};
use nit_tui::swarm::effective_max_swarm_size;

#[test]
fn fanout_maps_to_the_clamped_fork_width() {
    let ceiling = effective_max_swarm_size();
    let probes = [
        0usize,
        1,
        3,
        ceiling,
        ceiling + 1,
        ceiling.saturating_mul(4),
    ];
    for breadth in probes {
        let (policy, _) = all_multiway_policy("broadcast and rank", breadth);
        let want = clamp_fork_width(breadth, ceiling);
        assert_eq!(policy.k, want, "fan-out {breadth} clamps to {want}");
        assert!(policy.k >= 1, "a search always forks at least once");
        assert!(
            policy.k <= ceiling.max(1),
            "fan-out never exceeds the fd ceiling"
        );
    }
}

#[test]
fn the_round_is_one_explore_expansion() {
    let SearchPolicy { mood, k, budget } = all_multiway_policy("merge the branches", 4).0;
    let Budget {
        max_turns,
        max_nodes,
        max_tokens,
    } = budget;
    assert_eq!(mood, Mood::Explore, "@all keeps the whole fan-out alive");
    assert_eq!(
        max_turns, k,
        "one turn per fork bounds the search to a single round"
    );
    assert_eq!(
        max_nodes,
        k + 2,
        "root, k children, and the explore merge node"
    );
    assert_eq!(max_tokens, None, "@all leaves the token meter unbounded");
}

#[test]
fn the_body_becomes_an_integrate_task() {
    let (_, task) = all_multiway_policy("rewrite the parser", 2);
    assert_eq!(task.role, "integrate");
    assert_eq!(task.prompt, "rewrite the parser");
    // A multi-line body survives verbatim — the mapper never reflows the prompt.
    let (_, wrapped) = all_multiway_policy("line one\nline two", 2);
    assert_eq!(wrapped.prompt, "line one\nline two");
}
