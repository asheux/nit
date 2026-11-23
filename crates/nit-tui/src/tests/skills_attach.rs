use super::*;
use nit_core::{Buffer, GateMonitorSubView};

fn station() -> AppState {
    let mut state = AppState::new(
        ".".into(),
        Buffer::empty("file", None),
        Buffer::empty("notes", None),
    );
    state.focus = PaneId::GateMonitor;
    state.gate_monitor_sub_view = GateMonitorSubView::Skills;
    state
}

#[test]
fn attach_preserves_existing_composition_and_does_not_send() {
    let mut state = station();
    state.agents.chat_input = "@shadow review 🧪\nKeep this line".into();
    state.agents.chat_input_cursor = 5;
    state.agents.chat_input_selection_anchor = Some(7);
    state.agents.chat_prompt_history = vec!["previous prompt".into()];
    state.agents.chat_prompt_history_draft = Some("saved draft".into());
    state.agents.chat_prompt_history_pos = Some(0);
    state.agents.artifacts_popup_chat_input = "separate artifact draft".into();
    state.terminal_pane_active = true;
    attach(&mut state, "swarm-review");
    let draft = &state.agents.chat_input;
    assert!(draft.starts_with("@shadow review 🧪\nKeep this line\n"));
    assert!(draft.contains("nit skills swarm-review") && draft.contains("nit_skill"));
    assert_eq!(state.agents.chat_input_cursor, 5);
    assert_eq!(state.agents.chat_input_selection_anchor, Some(7));
    assert_eq!(state.agents.chat_prompt_history, ["previous prompt"]);
    assert_eq!(state.agents.chat_prompt_history_pos, Some(0));
    assert_eq!(
        state.agents.chat_prompt_history_draft.as_deref(),
        Some("saved draft")
    );
    assert_eq!(
        state.agents.artifacts_popup_chat_input,
        "separate artifact draft"
    );
    assert_eq!(state.focus, PaneId::Notes);
    assert_eq!(state.agents.console_tab, AgentConsoleTab::Thread);
    assert!(!state.terminal_pane_active);
    assert!(state.agents.messages.is_empty() && state.agents.active_turns.is_empty());
    assert!(
        state.agents.queued_codex_turns.is_empty() && state.agents.queued_claude_turns.is_empty()
    );
    let saved = draft.clone();
    state.focus = PaneId::GateMonitor;
    attach(&mut state, "swarm-review");
    assert_eq!(state.agents.chat_input, saved);
}

#[test]
fn attach_respects_shell_popup_and_pane_focus() {
    let mut state = station();
    state.terminal_popup.visible = true;
    attach(&mut state, "swarm-plan");
    assert_eq!(state.focus, PaneId::GateMonitor);
    assert!(state.agents.chat_input.is_empty());
    state.terminal_popup.visible = false;
    state.focus = PaneId::Editor;
    attach(&mut state, "swarm-plan");
    assert_eq!(state.focus, PaneId::Editor);
    assert!(state.agents.chat_input.is_empty());
}
