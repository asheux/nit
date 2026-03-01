use crate::state::{
    AgentAlert, AgentDiagnosticEvent, AgentLane, AgentMessage, AppState, McpStatus, MissionRecord,
};

/// Minimal event protocol for driving the Agent Station UI from an external runtime (Codex, Claude,
/// etc.). Intended to be transported as NDJSON over stdio or a socket.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentBusEvent {
    AgentUpsert { agent: AgentLane },
    MissionUpsert { mission: MissionRecord },
    MessageAppend { message: AgentMessage },
    AlertAppend { alert: AgentAlert },
    DiagnosticAppend { event: AgentDiagnosticEvent },
    McpStatus { status: McpStatus },
}

impl AgentBusEvent {
    pub fn apply(&self, state: &mut AppState) {
        match self {
            AgentBusEvent::AgentUpsert { agent } => {
                upsert_agent(state, agent.clone());
            }
            AgentBusEvent::MissionUpsert { mission } => {
                upsert_mission(state, mission.clone());
            }
            AgentBusEvent::MessageAppend { message } => {
                if let Some(mission_id) = message.mission_id.as_deref() {
                    mark_mission_provenance_dirty(state, mission_id);
                }
                state.agents.messages.push(message.clone());
                // If the operator was following the tail, keep following it.
                state.agents.console_scroll = usize::MAX;
            }
            AgentBusEvent::AlertAppend { alert } => {
                state.agents.alerts.push(alert.clone());
                state.agents.alert_selected = state
                    .agents
                    .alert_selected
                    .min(state.agents.alerts.len().saturating_sub(1));
            }
            AgentBusEvent::DiagnosticAppend { event } => {
                state.agents.diag_events.push(event.clone());
            }
            AgentBusEvent::McpStatus { status } => {
                state.agents.mcp = status.clone();
            }
        }

        // Drives the Agent ECG/criticality sampling and makes backend activity visible in the UI.
        state.agents.note_event();
    }
}

fn upsert_agent(state: &mut AppState, agent: AgentLane) {
    let Some(existing_idx) = state.agents.agents.iter().position(|a| a.id == agent.id) else {
        state.agents.agents.push(agent);
        if state.agents.selected_agent.is_none() {
            state.agents.selected_agent = state.agents.agents.last().map(|a| a.id.clone());
            state.agents.roster_selected = state.agents.agents.len().saturating_sub(1);
        }
        return;
    };
    state.agents.agents[existing_idx] = agent;
    if state.agents.selected_agent.is_none() {
        state.agents.selected_agent = state.agents.agents.get(0).map(|a| a.id.clone());
    }
    state.agents.roster_selected = state
        .agents
        .roster_selected
        .min(state.agents.agents.len().saturating_sub(1));
}

fn upsert_mission(state: &mut AppState, mission: MissionRecord) {
    let Some(existing_idx) = state
        .agents
        .missions
        .iter()
        .position(|m| m.id == mission.id)
    else {
        state.agents.missions.push(mission);
        if state.agents.selected_mission.is_none() {
            state.agents.selected_mission = state.agents.missions.last().map(|m| m.id.clone());
            state.agents.mission_selected = state.agents.missions.len().saturating_sub(1);
        }
        return;
    };
    state.agents.missions[existing_idx] = mission;

    if let Some(selected_id) = state.agents.selected_mission.as_deref() {
        if let Some(idx) = state
            .agents
            .missions
            .iter()
            .position(|m| m.id == selected_id)
        {
            state.agents.mission_selected = idx;
        }
    } else if !state.agents.missions.is_empty() {
        state.agents.selected_mission = state.agents.missions.get(0).map(|m| m.id.clone());
        state.agents.mission_selected = 0;
    }

    state.agents.mission_selected = state
        .agents
        .mission_selected
        .min(state.agents.missions.len().saturating_sub(1));
}

fn mark_mission_provenance_dirty(state: &mut AppState, mission_id: &str) {
    if state
        .agents
        .pending_provenance_mission_ids
        .iter()
        .all(|id| id != mission_id)
    {
        state
            .agents
            .pending_provenance_mission_ids
            .push(mission_id.to_string());
    }
}
