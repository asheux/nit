use super::{AppState, ClaudeRunner, CodexRunner};

pub(crate) fn chat_lane_runnable(state: &AppState, lane: &nit_core::AgentLane) -> bool {
    !lane.shadow
        && ((lane.is_codex() && state.agents.codex_cli_available)
            || (lane.is_claude() && state.agents.claude_cli_available))
}

pub(crate) fn chat_recipient(state: &AppState) -> Option<&str> {
    if state.agents.chat_needs_recipient {
        return None;
    }
    let id = state.agents.selected_agent.as_deref()?;
    state
        .agents
        .agents_get(id)
        .filter(|lane| chat_lane_runnable(state, lane))
        .map(|lane| lane.id.as_str())
}

pub(super) fn runner_available(
    state: &AppState,
    id: &str,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
) -> bool {
    state.agents.agents_get(id).is_some_and(|lane| {
        chat_lane_runnable(state, lane)
            && ((lane.is_codex() && codex.is_some()) || (lane.is_claude() && claude.is_some()))
    })
}

/// Only confirm ordinary sends that reached an active runner or its busy queue.
pub(super) fn accepted(state: &AppState, index: usize) -> bool {
    let agents = &state.agents;
    agents
        .queued_codex_turns
        .iter()
        .any(|q| q.prompt_msg_idx == Some(index))
        || agents
            .queued_claude_turns
            .iter()
            .any(|q| q.prompt_msg_idx == Some(index))
        || agents.active_turns.keys().any(|id| {
            agents.codex_turn_prompt_idx.get(id) == Some(&index)
                || agents.claude_turn_prompt_idx.get(id) == Some(&index)
        })
}
