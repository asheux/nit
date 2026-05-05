use nit_core::{AgentLaneKind, AppState};

use super::agent_id::PANE_SEPARATOR;
use crate::app::dispatch_agent_prompt;
use crate::claude_runner::ClaudeRunner;
use crate::codex_runner::CodexRunner;
use crate::vitals::VitalsState;

/// `NoSelection` lets the roster runtime turn an unselected dispatch
/// into a one-line system message rather than silently dropping it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum DispatchOutcome {
    Dispatched,
    NoSelection,
    PaneMissing,
}

/// `cwd` is intentionally absent: the pane's working directory is
/// resolved at dispatch time inside `resolve_dispatch_cwd`, not at
/// enqueue time, so a queue dequeue still picks up the correct cwd.
///
/// Retained for tests and for callers that bypass
/// `submit_chat_input_and_dispatch`'s parser. The runtime Enter handler
/// uses the canonical chat-input path via `with_pane_aliased`.
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
    if let Some(pane) = state
        .multipane
        .as_mut()
        .and_then(|mp| mp.panes.get_mut(pane_idx))
    {
        pane.has_run_mission = true;
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
    // Real swarm overlay wins; the synthetic chat id is the fallback so
    // every default-chat AgentMessage produced inside `body` carries a
    // pane-unique mission_id and the render filter can isolate it.
    state.agents.selected_mission = pane
        .mission_id
        .clone()
        .or_else(|| (!pane.chat_mission_id.is_empty()).then(|| pane.chat_mission_id.clone()));
    // Sentinel disables the global `selected_context_mission()` fallback so other panes' missions don't bleed in.
    let saved_mission_selected = std::mem::replace(&mut state.agents.mission_selected, usize::MAX);
    state.agents.swarm_default_template = pane.swarm_template.clone();
    state.agents.swarm_default_mission = pane.swarm_mission.clone();

    let result = body(state);

    mirror_back_pane(state, pane_idx);

    state.agents.chat_input = saved_chat_input;
    state.agents.chat_input_cursor = saved_chat_input_cursor;
    state.agents.chat_input_selection_anchor = saved_chat_input_anchor;
    state.agents.chat_input_scroll = saved_chat_input_scroll;
    state.agents.chat_prompt_history = saved_history;
    state.agents.chat_prompt_history_pos = saved_history_pos;
    state.agents.chat_prompt_history_draft = saved_history_draft;
    state.agents.selected_agent = saved_selected_agent;
    state.agents.selected_mission = saved_selected_mission;
    state.agents.mission_selected = saved_mission_selected;
    state.agents.swarm_default_template = saved_swarm_template;
    state.agents.swarm_default_mission = saved_swarm_mission;

    result
}

/// Mirror-back guard: a real swarm `mission_id` set by `@swarm` flows
/// through to `pane.mission_id` so subsequent aborts target it, but the
/// synthetic chat id (set by the alias source above) must NEVER be
/// written back — that would conflate "real swarm overlay" with
/// "default-chat fallback" and silently break swarm-followup
/// re-activation.
fn mirror_back_pane(state: &mut AppState, pane_idx: usize) {
    let new_selected = state.agents.selected_mission.clone();
    let Some(pane) = state
        .multipane
        .as_mut()
        .and_then(|mp| mp.panes.get_mut(pane_idx))
    else {
        return;
    };
    pane.chat_input = std::mem::take(&mut state.agents.chat_input);
    pane.chat_input_cursor = state.agents.chat_input_cursor;
    pane.chat_input_selection_anchor = state.agents.chat_input_selection_anchor;
    pane.chat_input_scroll = state.agents.chat_input_scroll;
    pane.chat_prompt_history = std::mem::take(&mut state.agents.chat_prompt_history);
    pane.chat_prompt_history_pos = state.agents.chat_prompt_history_pos;
    pane.chat_prompt_history_draft = state.agents.chat_prompt_history_draft.take();
    let synthetic_match = !pane.chat_mission_id.is_empty()
        && new_selected.as_deref() == Some(pane.chat_mission_id.as_str());
    if !synthetic_match {
        pane.mission_id = new_selected;
    }
}

pub(crate) fn bridge_pane_effort_to_runner_focused(state: &mut AppState, pane_idx: usize) {
    let Some(id) = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(pane_idx))
        .and_then(|pane| {
            if pane.agent_id.is_empty() {
                pane.selected_agent_id.clone()
            } else {
                Some(pane.agent_id.clone())
            }
        })
    else {
        return;
    };
    bridge_pane_effort_to_runner(state, pane_idx, &id);
}

/// One-way mirror: late operator size-clicks won't reach an in-flight
/// turn, matching the pre-fix behavior. The pane-local `selected_effort`
/// is copied under the materialised lane id so the dispatch contract
/// stays unchanged.
fn bridge_pane_effort_to_runner(state: &mut AppState, pane_idx: usize, materialised_id: &str) {
    let base_id = match materialised_id.split_once(PANE_SEPARATOR) {
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
    let target = if is_codex || matches!(kind, AgentLaneKind::Codex) {
        &mut state.agents.codex_selected_reasoning_effort
    } else if matches!(kind, AgentLaneKind::Claude) {
        &mut state.agents.claude_selected_effort
    } else {
        return;
    };
    target.insert(materialised_id.to_string(), effort);
}

#[cfg(test)]
#[path = "../tests/multipane_dispatch.rs"]
mod tests;
