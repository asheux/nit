//! Focused-pane dispatch helpers split out of `multipane::runtime`.
//!
//! `submit_focused_pane_input` is the canonical Enter handler for the
//! focused pane: it aliases pane state into `state.agents.*` via
//! `with_focused_pane_aliased`, then runs the global
//! `submit_chat_input_and_dispatch` so multipane inherits every chat
//! parser branch (`/abort`, `@swarm`, `@shadow`, `@all`, `@new`,
//! `@queue`, queueing, broadcast, swarm-followup re-activation, shadow
//! auto-enable, `push_chat_message`) without duplicating its logic.
//!
//! `pin_pane_chat_mission_on_lane` runs INSIDE the alias closure to
//! anchor the lane's `current_mission` on the pane's synthetic chat
//! id before dispatch — load-bearing for the breather isolation
//! filter that prevents pane A's lane status from leaking into pane B.

use nit_core::AppState;

use super::dispatch::{bridge_pane_effort_to_runner_focused, with_pane_aliased};
use super::runtime::{
    capture_pane_mission_ids, clear_focused_pane_input, focused_pane_agent_id, focused_pane_idx,
    focused_pane_mut, push_pane_system_message,
};
use crate::app::{parse_abort_command, submit_chat_input_and_dispatch};
use crate::claude_runner::ClaudeRunner;
use crate::codex_runner::CodexRunner;
use crate::shadow::ShadowRuntime;
use crate::swarm::SwarmRuntime;
use crate::vitals::VitalsState;

pub(crate) fn submit_focused_pane_input(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
) {
    let pane_idx = focused_pane_idx(state);
    let Some(pane) = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(pane_idx))
    else {
        return;
    };
    let chat_input = pane.chat_input.clone();
    if chat_input.trim().is_empty() {
        return;
    }
    let bound = !pane.agent_id.is_empty()
        || pane
            .selected_agent_id
            .as_deref()
            .is_some_and(|id| !id.is_empty());

    bridge_pane_effort_to_runner_focused(state, pane_idx);

    // Roster mode: only `/abort` is meaningful — fall through to the
    // alias path so the operator gets parity with chat aborts. Anything
    // else clears the input and posts a "no agent selected" notice.
    if !bound && parse_abort_command(&chat_input).is_none() {
        clear_focused_pane_input(state);
        push_pane_system_message(
            state,
            "no agent selected — press Ctrl+R to choose one".into(),
        );
        return;
    }

    with_focused_pane_aliased(state, |state| {
        if bound {
            pin_pane_chat_mission_on_lane(state, pane_idx);
        }
        let _ =
            submit_chat_input_and_dispatch(state, vitals, Some(codex), Some(claude), swarm, shadow);
    });
    if let Some(pane) = focused_pane_mut(state) {
        pane.has_run_mission = true;
    }
    capture_pane_mission_ids(state);
}

/// Wrapper around `with_pane_aliased` for the focused pane. Single
/// call site for chat-input editing and history nav so we don't
/// duplicate the focus lookup at every keystroke.
pub(super) fn with_focused_pane_aliased<R>(
    state: &mut AppState,
    body: impl FnOnce(&mut AppState) -> R,
) -> R {
    let pane_idx = focused_pane_idx(state);
    with_pane_aliased(state, pane_idx, body)
}

/// Pin the focused lane's `current_mission` to the pane's synthetic chat
/// id when no real swarm overlay exists. Locks the breather-filter
/// invariant `agent.current_mission == Some(mission_ctx)` for the render
/// alias even before `dispatch_agent_prompt` rewrites it on dispatch —
/// without this, a stale id from a prior swarm could survive long enough
/// to leak into another pane's render.
pub(super) fn pin_pane_chat_mission_on_lane(state: &mut AppState, pane_idx: usize) {
    let synthetic = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(pane_idx))
        .filter(|p| p.mission_id.is_none() && !p.chat_mission_id.is_empty())
        .map(|p| p.chat_mission_id.clone());
    let (Some(mid), Some(agent_id)) = (synthetic, focused_pane_agent_id(state)) else {
        return;
    };
    if let Some(lane) = state.agents.agents_get_mut(&agent_id) {
        lane.current_mission = Some(mid);
    }
}
