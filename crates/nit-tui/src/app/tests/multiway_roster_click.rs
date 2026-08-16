//! Phase 9 roster click routing (`mouse::apply_agent_ops_click_selection`).
//! A click on a multiway Mood / Mode row or a Live view / Graph button drives
//! the matching `AgentsState` field through the judge-frozen `agent_ops_view`
//! hit-tests. Because every `roster_multiway_*_line_idx` accessor returns `None`
//! while the flag is off, the routing is inert on the legacy roster — the
//! flag-off path stays byte-identical to the Template/Mission-only behaviour.

use super::*;

use nit_core::MultiwaySearchMood;

/// A roster-focused state with the multiway flag in the requested position.
fn roster_state(multiway_enabled: bool) -> AppState {
    let mut state = state_for_test();
    state.agents.dock_tab = nit_core::AgentOpsTab::Roster;
    state.agents.multiway_enabled = multiway_enabled;
    state
}

/// First column whose hit-test resolves to `want`, so a test targets a real
/// word cell without hard-coding the roster's exact column layout.
fn column_for<T: PartialEq>(hit: impl Fn(usize) -> Option<T>, want: &T) -> usize {
    (0..160)
        .find(|&col| hit(col).as_ref() == Some(want))
        .expect("a column maps to the requested option")
}

#[test]
fn mood_click_sets_the_default_search_mood() {
    let mut state = roster_state(true);
    let line =
        agent_ops_view::roster_multiway_mood_line_idx(&state).expect("mood row when enabled");
    let col = column_for(
        agent_ops_view::roster_multiway_mood_hit,
        &MultiwaySearchMood::Exploit,
    );

    apply_agent_ops_click_selection(&mut state, line, col, 80, &[]);

    assert_eq!(
        state.agents.multiway_default_mood,
        MultiwaySearchMood::Exploit
    );
    assert!(state.agents.roster_tree_selected.is_none());
}

#[test]
fn mode_click_turns_multiway_routing_on() {
    let mut state = roster_state(true);
    let line =
        agent_ops_view::roster_multiway_mode_line_idx(&state).expect("mode row when enabled");
    let col = column_for(agent_ops_view::roster_multiway_mode_hit, &true);

    apply_agent_ops_click_selection(&mut state, line, col, 80, &[]);

    assert!(state.agents.multiway_default_mode_on);
    assert!(state.agents.roster_tree_selected.is_none());
}

#[test]
fn live_view_button_toggles_the_popup() {
    let mut state = roster_state(true);
    let line =
        agent_ops_view::roster_multiway_buttons_line_idx(&state).expect("buttons row when enabled");
    let col = column_for(
        agent_ops_view::roster_multiway_button_hit,
        &agent_ops_view::RosterMultiwayButton::LiveView,
    );

    apply_agent_ops_click_selection(&mut state, line, col, 80, &[]);
    assert!(
        state.agents.show_multiway_popup,
        "first click opens the popup"
    );

    apply_agent_ops_click_selection(&mut state, line, col, 80, &[]);
    assert!(!state.agents.show_multiway_popup, "second click closes it");
}

#[test]
fn graph_button_requests_a_render() {
    let mut state = roster_state(true);
    let line =
        agent_ops_view::roster_multiway_buttons_line_idx(&state).expect("buttons row when enabled");
    let col = column_for(
        agent_ops_view::roster_multiway_button_hit,
        &agent_ops_view::RosterMultiwayButton::Graph,
    );

    apply_agent_ops_click_selection(&mut state, line, col, 80, &[]);

    assert!(state.agents.pending_multiway_graph);
}

#[test]
fn flag_off_routing_is_inert() {
    // Resolve the coordinates the *enabled* layout uses for the mood row, then
    // replay that exact click against a flag-off roster.
    let enabled = roster_state(true);
    let mood_line = agent_ops_view::roster_multiway_mood_line_idx(&enabled)
        .expect("enabled roster has a mood row");
    let mood_col = column_for(
        agent_ops_view::roster_multiway_mood_hit,
        &MultiwaySearchMood::Explore,
    );

    let mut state = roster_state(false);
    assert_eq!(agent_ops_view::roster_multiway_mood_line_idx(&state), None);
    assert_eq!(agent_ops_view::roster_multiway_mode_line_idx(&state), None);
    assert_eq!(
        agent_ops_view::roster_multiway_buttons_line_idx(&state),
        None
    );

    apply_agent_ops_click_selection(&mut state, mood_line, mood_col, 80, &[]);

    assert_eq!(
        state.agents.multiway_default_mood,
        MultiwaySearchMood::Balanced
    );
    assert!(!state.agents.multiway_default_mode_on);
    assert!(!state.agents.show_multiway_popup);
    assert!(!state.agents.pending_multiway_graph);
}
