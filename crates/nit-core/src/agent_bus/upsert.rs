use crate::state::{AgentLane, AppState, MissionRecord};

pub(super) fn upsert_agent(state: &mut AppState, agent: AgentLane) {
    let existing_idx = state
        .agents
        .agents_index
        .get(&agent.id)
        .copied()
        .or_else(|| state.agents.agents.iter().position(|a| a.id == agent.id));
    let Some(idx) = existing_idx else {
        let new_idx = state.agents.agents.len();
        let id = agent.id.clone();
        state.agents.agents.push(agent);
        state.agents.agents_index.insert(id, new_idx);
        if state.agents.selected_agent.is_none() {
            state.agents.selected_agent = state.agents.agents.last().map(|a| a.id.clone());
            state.agents.roster_selected = state.agents.agents.len().saturating_sub(1);
        }
        return;
    };
    state.agents.agents[idx] = agent;
    if state.agents.selected_agent.is_none() {
        state.agents.selected_agent = state.agents.agents.first().map(|a| a.id.clone());
    }
    state.agents.roster_selected = state
        .agents
        .roster_selected
        .min(state.agents.agents.len().saturating_sub(1));
}

pub(super) fn upsert_mission(state: &mut AppState, mission: MissionRecord) {
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
        state.agents.selected_mission = state.agents.missions.first().map(|m| m.id.clone());
        state.agents.mission_selected = 0;
    }

    state.agents.mission_selected = state
        .agents
        .mission_selected
        .min(state.agents.missions.len().saturating_sub(1));
}

fn push_unique_id(queue: &mut Vec<String>, id: &str) {
    if queue.iter().all(|existing| existing != id) {
        queue.push(id.to_string());
    }
}

pub(super) fn mark_mission_provenance_dirty(state: &mut AppState, mission_id: &str) {
    push_unique_id(&mut state.agents.pending_provenance_mission_ids, mission_id);
}

pub(super) fn mark_ad_hoc_provenance_dirty(state: &mut AppState, agent_id: &str) {
    push_unique_id(&mut state.agents.pending_provenance_agent_ids, agent_id);
}
