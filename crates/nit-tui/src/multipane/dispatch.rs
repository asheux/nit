use nit_core::{AgentLaneKind, AppState};

use crate::app::dispatch_agent_prompt;
use crate::claude_runner::ClaudeRunner;
use crate::codex_runner::CodexRunner;
use crate::vitals::VitalsState;

const MP_PANE_SUFFIX: &str = "#mp-pane-";

/// Outcome of `dispatch_pane_prompt`. The roster runtime turns
/// `NoSelection` into a one-line system message in the pane's chat
/// history rather than silently dropping the prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum DispatchOutcome {
    Dispatched,
    NoSelection,
    PaneMissing,
}

/// Dispatch the focused-pane prompt through the standard
/// `dispatch_agent_prompt` path. The pane's `cwd` is read at the dispatch
/// leaf via `app::dispatch::resolve_dispatch_cwd`, so this wrapper has no
/// `cwd` parameter — queue dequeues stay correct because the leaf
/// resolves at dispatch time, not enqueue time.
///
/// When the pane has no committed selection (`selected_agent_id == None`
/// and `agent_id` empty), returns `NoSelection` without dispatching so
/// the caller can post a "no agent selected" notice.
///
/// Retained for tests and for callers that want to bypass
/// `submit_chat_input_and_dispatch`'s parser. The runtime Enter handler
/// goes through the canonical chat-input path via
/// `with_pane_aliased`, not this function.
#[allow(dead_code)]
pub(crate) fn dispatch_pane_prompt(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    pane_idx: usize,
    prompt: String,
) -> DispatchOutcome {
    let Some(mp) = state.multipane.as_ref() else {
        return DispatchOutcome::PaneMissing;
    };
    let Some(pane) = mp.panes.get(pane_idx) else {
        return DispatchOutcome::PaneMissing;
    };
    let agent_id = pane
        .selected_agent_id
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| Some(pane.agent_id.clone()).filter(|s| !s.is_empty()));
    let Some(agent_id) = agent_id else {
        return DispatchOutcome::NoSelection;
    };
    let mission_id = pane.mission_id.clone();
    bridge_pane_effort_to_runner(state, pane_idx, &agent_id);
    dispatch_agent_prompt(state, vitals, codex, claude, agent_id, mission_id, prompt);
    if let Some(mp) = state.multipane.as_mut() {
        if let Some(pane) = mp.panes.get_mut(pane_idx) {
            pane.has_run_mission = true;
        }
    }
    DispatchOutcome::Dispatched
}

/// Lens-B aliasing: snap the pane's chat-input / history /
/// selected-agent / mission / swarm-default fields into
/// `state.agents.*`, run `body`, then mirror the resulting state back
/// onto the pane. This lets the canonical
/// `submit_chat_input_and_dispatch` (and its sibling editing /
/// abort / history helpers) drive multipane without duplicating their
/// parsing or dispatch logic.
///
/// Single-threaded run-loop guarantees only one alias at a time. Saved
/// values are restored on exit so unrelated state survives.
pub(crate) fn with_pane_aliased<R>(
    state: &mut AppState,
    pane_idx: usize,
    body: impl FnOnce(&mut AppState) -> R,
) -> R {
    let Some(pane) = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(pane_idx))
    else {
        return body(state);
    };

    let saved_chat_input = std::mem::take(&mut state.agents.chat_input);
    let saved_chat_input_cursor = state.agents.chat_input_cursor;
    let saved_chat_input_anchor = state.agents.chat_input_selection_anchor;
    let saved_chat_input_scroll = state.agents.chat_input_scroll;
    let saved_history = std::mem::take(&mut state.agents.chat_prompt_history);
    let saved_history_pos = state.agents.chat_prompt_history_pos;
    let saved_history_draft = state.agents.chat_prompt_history_draft.take();
    let saved_selected_agent = state.agents.selected_agent.clone();
    let saved_selected_mission = state.agents.selected_mission.clone();
    let saved_swarm_template = state.agents.swarm_default_template.clone();
    let saved_swarm_mission = state.agents.swarm_default_mission.clone();

    state.agents.chat_input = pane.chat_input.clone();
    state.agents.chat_input_cursor = pane.chat_input_cursor;
    state.agents.chat_input_selection_anchor = pane.chat_input_selection_anchor;
    state.agents.chat_input_scroll = pane.chat_input_scroll;
    state.agents.chat_prompt_history = pane.chat_prompt_history.clone();
    state.agents.chat_prompt_history_pos = pane.chat_prompt_history_pos;
    state.agents.chat_prompt_history_draft = pane.chat_prompt_history_draft.clone();
    state.agents.selected_agent = if !pane.agent_id.is_empty() {
        Some(pane.agent_id.clone())
    } else {
        pane.selected_agent_id.clone()
    };
    state.agents.selected_mission = pane.mission_id.clone();
    state.agents.swarm_default_template = pane.swarm_template.clone();
    state.agents.swarm_default_mission = pane.swarm_mission.clone();

    let result = body(state);

    if let Some(mp) = state.multipane.as_mut() {
        if let Some(pane) = mp.panes.get_mut(pane_idx) {
            pane.chat_input = std::mem::take(&mut state.agents.chat_input);
            pane.chat_input_cursor = state.agents.chat_input_cursor;
            pane.chat_input_selection_anchor = state.agents.chat_input_selection_anchor;
            pane.chat_input_scroll = state.agents.chat_input_scroll;
            pane.chat_prompt_history = std::mem::take(&mut state.agents.chat_prompt_history);
            pane.chat_prompt_history_pos = state.agents.chat_prompt_history_pos;
            pane.chat_prompt_history_draft = state.agents.chat_prompt_history_draft.take();
            // submit_chat_input_and_dispatch may have set selected_mission
            // (e.g. for a fresh @swarm); mirror it back so subsequent
            // aborts target the right mission.
            pane.mission_id = state.agents.selected_mission.clone();
        }
    }

    state.agents.chat_input = saved_chat_input;
    state.agents.chat_input_cursor = saved_chat_input_cursor;
    state.agents.chat_input_selection_anchor = saved_chat_input_anchor;
    state.agents.chat_input_scroll = saved_chat_input_scroll;
    state.agents.chat_prompt_history = saved_history;
    state.agents.chat_prompt_history_pos = saved_history_pos;
    state.agents.chat_prompt_history_draft = saved_history_draft;
    state.agents.selected_agent = saved_selected_agent;
    state.agents.selected_mission = saved_selected_mission;
    state.agents.swarm_default_template = saved_swarm_template;
    state.agents.swarm_default_mission = saved_swarm_mission;

    result
}

/// Bridge the focused pane's effort map to the global runner-side
/// reasoning-effort map. Wrapper around `bridge_pane_effort_to_runner`
/// that resolves the materialised id from the pane.
pub(crate) fn bridge_pane_effort_to_runner_focused(state: &mut AppState, pane_idx: usize) {
    let materialised_id = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(pane_idx))
        .and_then(|p| {
            if !p.agent_id.is_empty() {
                Some(p.agent_id.clone())
            } else {
                p.selected_agent_id.clone()
            }
        });
    let Some(id) = materialised_id else { return };
    bridge_pane_effort_to_runner(state, pane_idx, &id);
}

/// Mirror the focused pane's `selected_effort[base_id]` into the global
/// per-clone effort map under the materialised lane id so the runner
/// reads the pane-local choice without the dispatch contract changing.
/// One-way: late operator size-clicks won't reach an in-flight turn,
/// matching the pre-fix behavior.
fn bridge_pane_effort_to_runner(state: &mut AppState, pane_idx: usize, materialised_id: &str) {
    let base_id = match materialised_id.split_once(MP_PANE_SUFFIX) {
        Some((base, _)) => base.to_string(),
        None => materialised_id.to_string(),
    };
    let effort = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(pane_idx))
        .and_then(|p| p.selected_effort.get(&base_id))
        .cloned();
    let Some(effort) = effort else { return };
    let kind = state
        .agents
        .agents
        .iter()
        .find(|l| l.id == materialised_id || l.id == base_id)
        .map(|l| (l.is_codex(), l.kind));
    let Some((is_codex, kind)) = kind else { return };
    if is_codex || matches!(kind, AgentLaneKind::Codex) {
        state
            .agents
            .codex_selected_reasoning_effort
            .insert(materialised_id.to_string(), effort);
    } else if matches!(kind, AgentLaneKind::Claude) {
        state
            .agents
            .claude_selected_effort
            .insert(materialised_id.to_string(), effort);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nit_core::{MultipaneState, PaneSession};
    use std::path::PathBuf;

    fn fixture_state() -> AppState {
        let buffer = nit_core::Buffer::empty("scratch", None);
        let notes = nit_core::Buffer::empty("notes", None);
        let mut state = AppState::new(PathBuf::from("/workspace"), buffer, notes);
        state.multipane = Some(MultipaneState {
            backend_agent_id: String::new(),
            panes: vec![
                PaneSession {
                    pane_id: 0,
                    cwd: PathBuf::from("/pane0"),
                    ..PaneSession::default()
                },
                PaneSession {
                    pane_id: 1,
                    agent_id: "claude-haiku-4-5#mp-pane-01".into(),
                    cwd: PathBuf::from("/pane1"),
                    selected_agent_id: Some("claude-haiku-4-5#mp-pane-01".into()),
                    ..PaneSession::default()
                },
            ],
            focused: 0,
            grid_cols: 2,
            grid_rows: 1,
            backend_filter: None,
            help_open: false,
        });
        state
    }

    #[test]
    fn dispatch_no_selection_returns_marker_when_pane_unselected() {
        let mut state = fixture_state();
        let mut vitals = VitalsState::default();
        let outcome = dispatch_pane_prompt(&mut state, &mut vitals, None, None, 0, "hello".into());
        assert_eq!(outcome, DispatchOutcome::NoSelection);
    }

    #[test]
    fn dispatch_unknown_pane_returns_marker() {
        let mut state = fixture_state();
        let mut vitals = VitalsState::default();
        let outcome = dispatch_pane_prompt(&mut state, &mut vitals, None, None, 99, "hello".into());
        assert_eq!(outcome, DispatchOutcome::PaneMissing);
    }
}
