use std::path::PathBuf;

use nit_core::{AppState, MultipaneState, PaneSession};

use super::agent_id::pane_agent_id;
use super::grid::compute_grid_shape;
use crate::swarm::{copy_claude_runtime_metadata, copy_codex_runtime_metadata};

/// Install multipane state on `state`: replaces `state.agents.agents` with
/// N per-pane lanes (each cloned from the validated `backend` lane), copies
/// runtime metadata so each pane inherits the base context-window / effort
/// settings, and sets `state.multipane = Some(...)`.
///
/// Returns an error string if `backend` is not in the rostered lanes.
/// Callers must validate before invoking; this is the install-stage trust
/// boundary that protects `expect()` calls inside the function body.
pub fn install(
    state: &mut AppState,
    backend: &str,
    panes: usize,
    cwd: PathBuf,
) -> Result<(), String> {
    let panes = panes.max(1);
    let base_lane = state
        .agents
        .agents
        .iter()
        .find(|l| l.id == backend)
        .cloned()
        .ok_or_else(|| format!("backend '{backend}' not in roster"))?;

    let mut pane_sessions = Vec::with_capacity(panes);
    let mut pane_lanes = Vec::with_capacity(panes);
    for k in 0..panes {
        let agent_id = pane_agent_id(backend, k);
        let mut lane = base_lane.clone();
        lane.id = agent_id.clone();
        lane.role = agent_id.clone();
        pane_lanes.push(lane);
        pane_sessions.push(PaneSession {
            pane_id: k,
            agent_id,
            cwd: cwd.clone(),
            ..PaneSession::default()
        });
    }

    state.agents.agents = pane_lanes;
    state.agents.selected_agent = pane_sessions.first().map(|p| p.agent_id.clone());
    state.agents.roster_selected = 0;

    for pane in &pane_sessions {
        copy_codex_runtime_metadata(state, backend, &pane.agent_id);
        copy_claude_runtime_metadata(state, backend, &pane.agent_id);
    }

    let (grid_cols, grid_rows) = compute_grid_shape(panes);
    state.multipane = Some(MultipaneState {
        backend_agent_id: backend.to_string(),
        panes: pane_sessions,
        focused: 0,
        grid_cols,
        grid_rows,
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nit_core::{AgentLane, AgentLaneKind, AgentStatus, AgentsState};

    fn fixture_state(backend_id: &str) -> AppState {
        let buffer = nit_core::Buffer::empty("scratch", None);
        let notes = nit_core::Buffer::empty("notes", None);
        let mut state = AppState::new(PathBuf::from("/tmp"), buffer, notes);
        state.agents = AgentsState::default();
        state.agents.agents.push(AgentLane {
            id: backend_id.into(),
            role: backend_id.into(),
            lane: "Claude".into(),
            kind: AgentLaneKind::Claude,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
            shadow: false,
        });
        state
    }

    #[test]
    fn install_replaces_lanes_and_sets_multipane() {
        let mut state = fixture_state("claude-haiku-4-5");
        install(&mut state, "claude-haiku-4-5", 4, PathBuf::from("/work")).expect("ok");

        let mp = state.multipane.as_ref().expect("set");
        assert_eq!(mp.panes.len(), 4);
        assert_eq!(mp.backend_agent_id, "claude-haiku-4-5");
        assert_eq!(mp.grid_cols, 2);
        assert_eq!(mp.grid_rows, 2);
        for (i, pane) in mp.panes.iter().enumerate() {
            assert_eq!(pane.pane_id, i);
            assert_eq!(pane.agent_id, format!("claude-haiku-4-5#mp-pane-{i:02}"));
            assert_eq!(pane.cwd, PathBuf::from("/work"));
        }
        assert_eq!(state.agents.agents.len(), 4);
    }

    #[test]
    fn install_returns_err_when_backend_missing() {
        let mut state = fixture_state("claude-haiku-4-5");
        let result = install(&mut state, "bogus", 4, PathBuf::from("/work"));
        assert!(result.is_err());
        assert!(state.multipane.is_none());
    }
}
