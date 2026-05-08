//! `:help`, `?`, and the substrate-overlay UI commands — popup open/close
//! plumbing routed through `handle_command_line` and `apply_action`.

use super::*;

#[test]
fn command_help_dash_question_opens_help_popup() {
    let (_root, mut state) = empty_state("cmd-help-dash");
    assert!(!state.show_help);
    assert!(!handle_command_line(&mut state, "help - ?"));
    assert!(state.show_help);
    assert_eq!(state.help_scroll, 0);
}

#[test]
fn command_question_mark_opens_help_popup() {
    let (_root, mut state) = empty_state("cmd-help-qmark");
    assert!(!state.show_help);
    assert!(!handle_command_line(&mut state, "?"));
    assert!(state.show_help);
    assert_eq!(state.help_scroll, 0);
}

#[test]
fn command_colon_help_dash_question_opens_help_with_file_tree_open() {
    let (_root, mut state) = empty_state("cmd-help-colon-tree");
    state.file_tree.open = true;
    assert!(!state.show_help);
    assert!(!handle_command_line(&mut state, ":help - ?"));
    assert!(state.show_help);
    assert_eq!(state.help_scroll, 0);
}

#[test]
fn substrate_overlay_toggle_cycles_through_three_tabs() {
    let (_root, mut state) = empty_state("substrate-overlay-toggle");
    assert_eq!(state.substrate_overlay_tab, SubstrateOverlayTab::Signals);
    let _ = apply_action(&mut state, Action::SubstrateOverlayToggleTab);
    assert_eq!(state.substrate_overlay_tab, SubstrateOverlayTab::Claims);
    let _ = apply_action(&mut state, Action::SubstrateOverlayToggleTab);
    assert_eq!(
        state.substrate_overlay_tab,
        SubstrateOverlayTab::Assumptions
    );
    let _ = apply_action(&mut state, Action::SubstrateOverlayToggleTab);
    assert_eq!(state.substrate_overlay_tab, SubstrateOverlayTab::Signals);
}

#[test]
fn show_substrate_opens_overlay_and_resets_scroll() {
    let (_root, mut state) = empty_state("substrate-overlay-show");
    state.substrate_overlay_scroll = 42;
    let _ = apply_action(&mut state, Action::ShowSubstrate);
    assert!(state.show_substrate_overlay);
    assert_eq!(state.substrate_overlay_scroll, 0);
}

#[test]
fn hide_substrate_closes_overlay() {
    let (_root, mut state) = empty_state("substrate-overlay-hide");
    state.show_substrate_overlay = true;
    let _ = apply_action(&mut state, Action::HideSubstrate);
    assert!(!state.show_substrate_overlay);
}
