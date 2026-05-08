use std::time::Instant;

use crate::state::{
    AgentAlertSeverity, AgentDiagnosticEvent, AgentStatus, AgentTurnState, AppState,
};

use super::helpers::{backend_source_for_agent, timestamp_label};

pub(super) fn handle_turn_started(
    state: &mut AppState,
    agent_id: &str,
    mission_id: &Option<String>,
) {
    let now = Instant::now();
    state.agents.active_turns.insert(
        agent_id.to_string(),
        AgentTurnState {
            started_at: now,
            last_heartbeat_at: now,
            last_output_at: now,
            stage: None,
        },
    );
    let at = timestamp_label(state);
    if let Some(agent) = state.agents.agents_get_mut(agent_id) {
        agent.status = AgentStatus::Running;
        agent.queue_len = agent.queue_len.max(1);
        agent.heartbeat_age_secs = 0;
        agent.current_mission = mission_id.clone();
    }

    if let Some(mid) = mission_id.as_deref() {
        if let Some(mission) = state
            .agents
            .missions
            .iter_mut()
            .find(|mission| mission.id == mid)
        {
            mission.status = "RUNNING".into();
            mission.updated_at = at;
        }
    }

    // Genome baseline capture: on a fresh per-agent retry (count == 0), seed
    // any missing baselines from current reports. Never overwrite existing
    // baselines — in parallel mode a sibling agent's in-flight retry may
    // still be comparing against the original snapshot for that file.
    let fresh_turn = state
        .genome_retry_counts
        .get(agent_id)
        .copied()
        .unwrap_or(0)
        == 0;
    if fresh_turn {
        for (path, report) in state.genome_reports.iter() {
            state
                .genome_baselines
                .entry(path.clone())
                .or_insert_with(|| report.clone());
        }
    }
    state
        .genome_turn_modified
        .entry(agent_id.to_string())
        .or_default()
        .clear();
    state.genome_shadow_evals.clear();
    state.genome_turn_active.insert(agent_id.to_string());
}

pub(super) fn handle_turn_heartbeat(
    state: &mut AppState,
    agent_id: &str,
    mission_id: &Option<String>,
) {
    if let Some(turn) = state.agents.active_turns.get_mut(agent_id) {
        turn.last_heartbeat_at = Instant::now();
    }
    if let Some(agent) = state.agents.agents_get_mut(agent_id) {
        agent.heartbeat_age_secs = 0;
        agent.current_mission = mission_id.clone();
    }
}

pub(super) fn handle_turn_stage(
    state: &mut AppState,
    agent_id: &str,
    mission_id: &Option<String>,
    stage: &str,
) {
    if let Some(turn) = state.agents.active_turns.get_mut(agent_id) {
        turn.last_output_at = Instant::now();
        turn.stage = Some(stage.to_string());
    }
    if let Some(agent) = state.agents.agents_get_mut(agent_id) {
        agent.current_mission = mission_id.clone();
    }
}

pub(super) fn handle_turn_log(state: &mut AppState, agent_id: &str, message: &str) {
    let source = backend_source_for_agent(state, agent_id);
    if let Some(turn) = state.agents.active_turns.get_mut(agent_id) {
        turn.last_output_at = Instant::now();
    }
    let lowered = message.to_ascii_lowercase();
    let severity = if lowered.contains("error") || lowered.contains("failed") {
        AgentAlertSeverity::Warn
    } else {
        AgentAlertSeverity::Info
    };
    state.agents.diag_events.push(AgentDiagnosticEvent {
        severity,
        source: source.into(),
        message: format!("[{agent_id}] {message}"),
        at: timestamp_label(state),
    });
}
