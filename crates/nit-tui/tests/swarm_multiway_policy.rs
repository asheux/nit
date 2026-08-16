//! Phase 7 mapping: a cleaned `@swarm` command (int-route strips the leading
//! `mode=multiway` token upstream) maps onto a multiway `SearchPolicy` + `Task`.
//! The default `@swarm` parser must stay byte-identical — `swarm/command.rs` is
//! untouched, so `mode=multiway` is never a recognised flag.

use nit_tui::multiway::runtime::clamp_fork_width;
use nit_tui::multiway::{Mood, SearchPolicy, Task};
use nit_tui::swarm::{effective_max_swarm_size, parse_swarm_command, swarm_multiway_policy};

fn policy_for(command: &str) -> (SearchPolicy, Task) {
    let parsed = parse_swarm_command(command).expect("valid @swarm command");
    swarm_multiway_policy(&parsed)
}

#[test]
fn template_selects_search_mood() {
    for (flag, expected) in [
        ("template=parallel", Mood::Explore),
        ("template=bulk", Mood::Explore),
        ("template=lab", Mood::Balanced),
    ] {
        let (policy, _) = policy_for(&format!("@swarm {flag} ship it"));
        assert_eq!(policy.mood, expected, "{flag}");
    }
}

#[test]
fn size_sets_clamped_fork_width() {
    let ceiling = effective_max_swarm_size();

    let defaulted = policy_for("@swarm ship it").0;
    assert_eq!(
        defaulted.k,
        clamp_fork_width(SearchPolicy::default().k, ceiling)
    );

    let counted = policy_for("@swarm 5 ship it").0;
    assert_eq!(counted.k, clamp_fork_width(5, ceiling));

    let saturated = policy_for("@swarm all ship it").0;
    assert_eq!(saturated.k, ceiling.max(1));

    // A request past the ceiling is clamped down rather than left to exhaust fds.
    let over_request = policy_for(&format!("@swarm {} ship it", ceiling + 1000)).0;
    assert_eq!(over_request.k, ceiling.max(1));
}

#[test]
fn mission_becomes_task_role() {
    let (_, research) = policy_for("@swarm mission=research dig in");
    assert_eq!(research.role, "research");

    let (_, computational) = policy_for("@swarm mission=computational-research simulate");
    assert_eq!(computational.role, "computational-research");

    // A general mission and an unspecified one both fall back to the writer role
    // the other multiway front-ends use.
    let (_, general) = policy_for("@swarm mission=general build");
    assert_eq!(general.role, "integrate");
    let (_, unspecified) = policy_for("@swarm build");
    assert_eq!(unspecified.role, "integrate");
}

#[test]
fn prompt_becomes_the_search_task() {
    let (_, task) = policy_for("@swarm 3 template=lab mission=research refactor the parser");
    assert_eq!(task.prompt, "refactor the parser");
}

#[test]
fn default_swarm_parser_keeps_mode_token_as_prose() {
    // `mode=multiway` is not a swarm flag: the unmodified parser leaves it in the
    // prompt and never sets a template/mission. int-route strips the token before
    // parsing, so the default @swarm path is unchanged by Phase 7.
    let parsed = parse_swarm_command("@swarm mode=multiway do the work").expect("valid command");
    assert_eq!(parsed.prompt, "mode=multiway do the work");
    assert_eq!(parsed.template, None);
    assert_eq!(parsed.mission_kind, None);
}
