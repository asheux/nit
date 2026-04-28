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
    // Snapshot the pane's template/mission AT DISPATCH TIME so a later
    // operator switch in the roster doesn't perturb an in-flight mission.
    // TODO(multipane phase 4+): thread `_template_snapshot` /
    // `_mission_snapshot` into the swarm orchestrator once multipane
    // routes through `parse_swarm_command` instead of the direct
    // `dispatch_agent_prompt` shortcut.
    let _template_snapshot = pane.swarm_template.clone();
    let _mission_snapshot = pane.swarm_mission.clone();
    bridge_pane_effort_to_runner(state, pane_idx, &agent_id);
    dispatch_agent_prompt(state, vitals, codex, claude, agent_id, mission_id, prompt);
    if let Some(mp) = state.multipane.as_mut() {
        if let Some(pane) = mp.panes.get_mut(pane_idx) {
            pane.has_run_mission = true;
        }
    }
    DispatchOutcome::Dispatched
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
