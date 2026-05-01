use std::path::PathBuf;

use nit_core::{AppState, MultipaneState, PaneSession};

use super::agent_id::{pane_agent_id, pane_chat_mission_id};
use super::grid::compute_grid_shape;
use crate::swarm::{copy_claude_runtime_metadata, copy_codex_runtime_metadata};

/// Family aliases recognised by `--backend`. Anything else is treated as a
/// specific lane id (and validated against the rostered set in
/// `main::run_multipane`). Closed set so a future model literally named
/// `claude` cannot collide with the family alias.
pub const BACKEND_FAMILIES: &[&str] = &["codex", "claude", "gemini", "local"];

/// True when `value` matches one of `BACKEND_FAMILIES` (case-insensitive).
pub fn is_backend_family(value: &str) -> bool {
    BACKEND_FAMILIES
        .iter()
        .any(|fam| value.eq_ignore_ascii_case(fam))
}

/// Install multipane state on `state`. Three modes routed by `backend`:
///
/// - `Some(specific-id)` — pre-pick path: every pane is born with
///   `selected_agent_id = Some("<id>#mp-pane-NN")`, the lane is cloned
///   into `state.agents.agents` and runtime metadata is copied so each
///   pane inherits the base context-window / effort settings. Mirrors the
///   pre-existing `install` behaviour.
/// - `Some(family)` — filter path: panes start in roster mode showing
///   only that family's lanes. `state.agents.agents` is left intact.
/// - `None` — full roster path: panes start in roster mode showing every
///   available backend. `state.agents.agents` is left intact.
///
/// Returns an error string only on pre-pick when the specific id is not
/// in the roster. Family / full-roster modes always succeed because the
/// empty-state row is rendered at draw time.
pub fn install_filtered(
    state: &mut AppState,
    backend: Option<&str>,
    panes: usize,
    cwd: PathBuf,
) -> Result<(), String> {
    let panes = panes.max(1);
    let pre_pick_lane = match backend {
        Some(value) if !is_backend_family(value) => {
            let base = state
                .agents
                .agents
                .iter()
                .find(|l| l.id == value)
                .cloned()
                .ok_or_else(|| format!("backend '{value}' not in roster"))?;
            Some(base)
        }
        _ => None,
    };

    let seed_template = state.agents.swarm_default_template.clone();
    let seed_mission = state.agents.swarm_default_mission.clone();
    let mut pane_sessions = Vec::with_capacity(panes);
    if let Some(base_lane) = pre_pick_lane {
        let mut pane_lanes = Vec::with_capacity(panes);
        for k in 0..panes {
            let agent_id = pane_agent_id(&base_lane.id, k);
            let mut lane = base_lane.clone();
            lane.id = agent_id.clone();
            lane.role = agent_id.clone();
            pane_lanes.push(lane);
            pane_sessions.push(PaneSession {
                pane_id: k,
                agent_id: agent_id.clone(),
                cwd: cwd.clone(),
                selected_agent_id: Some(agent_id),
                swarm_template: seed_template.clone(),
                swarm_mission: seed_mission.clone(),
                chat_mission_id: pane_chat_mission_id(k),
                ..PaneSession::default()
            });
        }
        state.agents.agents.extend(pane_lanes);
        state.agents.rebuild_agents_index();
        state.agents.selected_agent = pane_sessions.first().map(|p| p.agent_id.clone());
        for pane in &pane_sessions {
            copy_codex_runtime_metadata(state, &base_lane.id, &pane.agent_id);
            copy_claude_runtime_metadata(state, &base_lane.id, &pane.agent_id);
        }
    } else {
        for k in 0..panes {
            pane_sessions.push(PaneSession {
                pane_id: k,
                cwd: cwd.clone(),
                swarm_template: seed_template.clone(),
                swarm_mission: seed_mission.clone(),
                chat_mission_id: pane_chat_mission_id(k),
                ..PaneSession::default()
            });
        }
    }

    let backend_filter = backend.map(|s| s.to_string());
    let backend_agent_id = backend_filter.clone().unwrap_or_default();
    let (grid_cols, grid_rows) = compute_grid_shape(panes);
    state.multipane = Some(MultipaneState {
        backend_agent_id,
        panes: pane_sessions,
        focused: 0,
        grid_cols,
        grid_rows,
        backend_filter,
        help_open: false,
    });

    Ok(())
}

/// Materialise the per-pane lane for `selected_base` on the focused pane,
/// copying runtime metadata so the new lane inherits context-window /
/// effort settings. Idempotent: if the lane id already exists in
/// `state.agents.agents`, no clone is added but the pane's
/// `selected_agent_id` / `agent_id` are still updated to match.
///
/// Returns the materialised pane-suffixed agent id.
pub fn materialise_pane_lane(
    state: &mut AppState,
    pane_idx: usize,
    selected_base: &str,
) -> Option<String> {
    let pane_id_value = pane_agent_id(selected_base, pane_idx);

    let base_lane = state
        .agents
        .agents
        .iter()
        .find(|l| l.id == selected_base)
        .cloned()?;
    let already_present = state.agents.agents_get(&pane_id_value).is_some();
    if !already_present {
        let mut lane = base_lane.clone();
        lane.id = pane_id_value.clone();
        lane.role = pane_id_value.clone();
        let new_idx = state.agents.agents.len();
        state.agents.agents.push(lane);
        state
            .agents
            .agents_index
            .insert(pane_id_value.clone(), new_idx);
        copy_codex_runtime_metadata(state, selected_base, &pane_id_value);
        copy_claude_runtime_metadata(state, selected_base, &pane_id_value);
    }

    if let Some(mp) = state.multipane.as_mut() {
        if let Some(pane) = mp.panes.get_mut(pane_idx) {
            pane.agent_id = pane_id_value.clone();
            pane.selected_agent_id = Some(pane_id_value.clone());
        }
    }

    Some(pane_id_value)
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

    fn fixture_state_with_two_backends() -> AppState {
        let mut state = fixture_state("claude-haiku-4-5");
        state.agents.agents.push(AgentLane {
            id: "gpt-5".into(),
            role: "gpt-5".into(),
            lane: "Codex".into(),
            kind: AgentLaneKind::Codex,
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
    fn install_pre_picks_when_specific_backend() {
        let mut state = fixture_state("claude-haiku-4-5");
        install_filtered(
            &mut state,
            Some("claude-haiku-4-5"),
            4,
            PathBuf::from("/work"),
        )
        .expect("ok");

        let mp = state.multipane.as_ref().expect("set");
        assert_eq!(mp.panes.len(), 4);
        assert_eq!(mp.backend_filter.as_deref(), Some("claude-haiku-4-5"));
        for (i, pane) in mp.panes.iter().enumerate() {
            assert_eq!(pane.pane_id, i);
            let want = format!("claude-haiku-4-5#mp-pane-{i:02}");
            assert_eq!(pane.agent_id, want);
            assert_eq!(pane.selected_agent_id.as_deref(), Some(want.as_str()));
            assert_eq!(pane.cwd, PathBuf::from("/work"));
        }
        assert_eq!(state.agents.agents.len(), 5);
    }

    #[test]
    fn install_returns_err_when_specific_backend_missing() {
        let mut state = fixture_state("claude-haiku-4-5");
        let result = install_filtered(&mut state, Some("bogus"), 4, PathBuf::from("/work"));
        assert!(result.is_err());
        assert!(state.multipane.is_none());
    }

    #[test]
    fn install_filtered_with_no_backend_leaves_roster_intact() {
        let mut state = fixture_state_with_two_backends();
        let canonical_len = state.agents.agents.len();
        install_filtered(&mut state, None, 4, PathBuf::from("/work")).expect("ok");

        let mp = state.multipane.as_ref().expect("set");
        assert_eq!(mp.panes.len(), 4);
        assert!(mp.backend_filter.is_none());
        assert_eq!(state.agents.agents.len(), canonical_len);
        for pane in &mp.panes {
            assert!(pane.selected_agent_id.is_none());
            assert!(pane.agent_id.is_empty());
        }
    }

    #[test]
    fn install_filtered_with_family_backend_records_filter_only() {
        let mut state = fixture_state_with_two_backends();
        let canonical_len = state.agents.agents.len();
        install_filtered(&mut state, Some("claude"), 4, PathBuf::from("/work")).expect("ok");

        let mp = state.multipane.as_ref().expect("set");
        assert_eq!(mp.backend_filter.as_deref(), Some("claude"));
        assert_eq!(state.agents.agents.len(), canonical_len);
        for pane in &mp.panes {
            assert!(pane.selected_agent_id.is_none());
            assert!(pane.agent_id.is_empty());
        }
    }

    #[test]
    fn materialise_pane_lane_creates_unique_lane_per_pane() {
        let mut state = fixture_state_with_two_backends();
        install_filtered(&mut state, None, 2, PathBuf::from("/work")).expect("ok");

        let id_a = materialise_pane_lane(&mut state, 0, "claude-haiku-4-5").expect("id 0");
        let id_b = materialise_pane_lane(&mut state, 1, "claude-haiku-4-5").expect("id 1");
        assert_ne!(id_a, id_b);
        assert!(state.agents.agents.iter().any(|l| l.id == id_a));
        assert!(state.agents.agents.iter().any(|l| l.id == id_b));

        let mp = state.multipane.as_ref().expect("set");
        assert_eq!(
            mp.panes[0].selected_agent_id.as_deref(),
            Some(id_a.as_str())
        );
        assert_eq!(
            mp.panes[1].selected_agent_id.as_deref(),
            Some(id_b.as_str())
        );
    }

    #[test]
    fn materialise_pane_lane_idempotent_on_repeat() {
        let mut state = fixture_state_with_two_backends();
        install_filtered(&mut state, None, 2, PathBuf::from("/work")).expect("ok");
        let lanes_before_first = state.agents.agents.len();

        let _ = materialise_pane_lane(&mut state, 0, "claude-haiku-4-5");
        let after_first = state.agents.agents.len();
        let _ = materialise_pane_lane(&mut state, 0, "claude-haiku-4-5");
        let after_second = state.agents.agents.len();

        assert_eq!(after_first, lanes_before_first + 1);
        assert_eq!(after_second, after_first);
    }

    #[test]
    fn is_backend_family_recognises_closed_set_case_insensitive() {
        for fam in ["codex", "Claude", "GEMINI", "local"] {
            assert!(is_backend_family(fam), "{fam} should be a family");
        }
        for not_fam in ["claude-haiku-4-5", "gpt-5", "anthropic", ""] {
            assert!(
                !is_backend_family(not_fam),
                "{not_fam} should not be a family"
            );
        }
    }
}
