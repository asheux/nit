use std::collections::HashSet;

use nit_core::{AgentStatus, AppState};

use super::{
    effective_max_swarm_size, SwarmSize, SwarmTemplate, DEFAULT_SWARM_SIZE, MAX_SWARM_SIZE,
};

pub(super) fn swarm_clone_base_id(agent_id: &str) -> Option<&str> {
    agent_id.split_once("#swarm-").map(|(base_id, _)| base_id)
}

pub(super) fn is_swarm_clone_agent_id(agent_id: &str) -> bool {
    swarm_clone_base_id(agent_id).is_some()
}

pub fn chat_clone_base_id(agent_id: &str) -> Option<&str> {
    agent_id.split_once("#chat-clone-").map(|(base, _)| base)
}

pub fn is_chat_clone_agent_id(agent_id: &str) -> bool {
    chat_clone_base_id(agent_id).is_some()
}

pub fn is_any_clone_agent_id(agent_id: &str) -> bool {
    is_swarm_clone_agent_id(agent_id) || is_chat_clone_agent_id(agent_id)
}

/// Display-only: compact `base#swarm-mis-XXX-clone-NN` to `base#clone-NN`.
pub fn compact_agent_display_id(agent_id: &str) -> String {
    if let Some((base, rest)) = agent_id.split_once("#swarm-") {
        // rest is e.g. "mis-002-clone-01"; extract "clone-NN" suffix.
        if let Some(clone_pos) = rest.find("clone-") {
            return format!("{base}#{}", &rest[clone_pos..]);
        }
    }
    agent_id.to_string()
}

pub(super) fn is_swarm_clone_for_mission(agent_id: &str, mission_id: &str) -> bool {
    let Some((_base_id, rest)) = agent_id.split_once("#swarm-") else {
        return false;
    };
    rest.strip_prefix(mission_id)
        .is_some_and(|suffix| suffix.starts_with('-'))
}

/// Propagate Codex runtime metadata (context window, reasoning efforts) from
/// the base agent lane to a swarm/chat clone lane so the clone inherits the
/// same token accounting and selected-effort UX without an extra probe.
pub(crate) fn copy_codex_runtime_metadata(state: &mut AppState, base_id: &str, clone_id: &str) {
    if let Some(tokens) = state
        .agents
        .codex_effective_context_window_tokens
        .get(base_id)
        .copied()
    {
        state
            .agents
            .codex_effective_context_window_tokens
            .insert(clone_id.to_string(), tokens);
    }
    if let Some(effort) = state
        .agents
        .codex_default_reasoning_effort
        .get(base_id)
        .cloned()
    {
        state
            .agents
            .codex_default_reasoning_effort
            .insert(clone_id.to_string(), effort);
    }
    if let Some(efforts) = state
        .agents
        .codex_supported_reasoning_efforts
        .get(base_id)
        .cloned()
    {
        state
            .agents
            .codex_supported_reasoning_efforts
            .insert(clone_id.to_string(), efforts);
    }
    if let Some(effort) = state
        .agents
        .codex_selected_reasoning_effort
        .get(base_id)
        .cloned()
    {
        state
            .agents
            .codex_selected_reasoning_effort
            .insert(clone_id.to_string(), effort);
    }
}

/// Propagate Claude runtime metadata (context window, effort selection) from
/// the base agent lane to a swarm/chat clone lane so the clone inherits the
/// same token accounting and selected-effort UX without an extra probe.
pub(crate) fn copy_claude_runtime_metadata(state: &mut AppState, base_id: &str, clone_id: &str) {
    if let Some(tokens) = state
        .agents
        .claude_effective_context_window_tokens
        .get(base_id)
        .copied()
    {
        state
            .agents
            .claude_effective_context_window_tokens
            .insert(clone_id.to_string(), tokens);
    }
    if let Some(effort) = state.agents.claude_default_effort.get(base_id).cloned() {
        state
            .agents
            .claude_default_effort
            .insert(clone_id.to_string(), effort);
    }
    if let Some(efforts) = state.agents.claude_supported_efforts.get(base_id).cloned() {
        state
            .agents
            .claude_supported_efforts
            .insert(clone_id.to_string(), efforts);
    }
    if let Some(effort) = state.agents.claude_selected_effort.get(base_id).cloned() {
        state
            .agents
            .claude_selected_effort
            .insert(clone_id.to_string(), effort);
    }
}

pub(crate) fn insert_swarm_clone_lane(
    state: &mut AppState,
    base_id: &str,
    clone_lane: nit_core::AgentLane,
) {
    if state
        .agents
        .agents
        .iter()
        .any(|existing| existing.id == clone_lane.id)
    {
        return;
    }

    let Some(base_pos) = state
        .agents
        .agents
        .iter()
        .position(|lane| lane.id == base_id)
    else {
        state.agents.agents.push(clone_lane);
        return;
    };

    let mut insert_pos = base_pos.saturating_add(1);
    while insert_pos < state.agents.agents.len() {
        let lane = &state.agents.agents[insert_pos];
        if swarm_clone_base_id(lane.id.as_str()) == Some(base_id)
            || chat_clone_base_id(lane.id.as_str()) == Some(base_id)
        {
            insert_pos = insert_pos.saturating_add(1);
        } else {
            break;
        }
    }
    state.agents.agents.insert(insert_pos, clone_lane);
}

/// Drain all queued Codex and Claude turns for a specific agent, decrementing
/// `queue_len` for each removed turn. Used when a task agent fails during
/// swarm execution so that orphaned queued turns don't leak.
pub(super) fn drain_queued_turns_for_agent(state: &mut AppState, agent_id: &str) {
    let codex_removed = state
        .agents
        .queued_codex_turns
        .iter()
        .filter(|t| t.agent_id == agent_id)
        .count();
    state
        .agents
        .queued_codex_turns
        .retain(|t| t.agent_id != agent_id);

    let claude_removed = state
        .agents
        .queued_claude_turns
        .iter()
        .filter(|t| t.agent_id == agent_id)
        .count();
    state
        .agents
        .queued_claude_turns
        .retain(|t| t.agent_id != agent_id);

    let total_removed = codex_removed + claude_removed;
    if total_removed > 0 {
        if let Some(agent) = state.agents.agents.iter_mut().find(|a| a.id == agent_id) {
            agent.queue_len = agent.queue_len.saturating_sub(total_removed);
        }
    }
}

pub(super) fn cleanup_swarm_clones_for_mission(state: &mut AppState, mission_id: &str) {
    let clone_ids = state
        .agents
        .agents
        .iter()
        .filter(|lane| is_swarm_clone_for_mission(lane.id.as_str(), mission_id))
        .map(|lane| lane.id.clone())
        .collect::<HashSet<_>>();
    if clone_ids.is_empty() {
        return;
    }

    // Decrement queue_len for each Codex turn that will be removed, while agents still exist.
    for turn in state.agents.queued_codex_turns.iter() {
        if clone_ids.contains(turn.agent_id.as_str()) {
            if let Some(agent) = state
                .agents
                .agents
                .iter_mut()
                .find(|a| a.id == turn.agent_id)
            {
                agent.queue_len = agent.queue_len.saturating_sub(1);
            }
        }
    }
    state
        .agents
        .queued_codex_turns
        .retain(|turn| !clone_ids.contains(turn.agent_id.as_str()));

    // Decrement queue_len for each Claude turn that will be removed, while agents still exist.
    for turn in state.agents.queued_claude_turns.iter() {
        if clone_ids.contains(turn.agent_id.as_str()) {
            if let Some(agent) = state
                .agents
                .agents
                .iter_mut()
                .find(|a| a.id == turn.agent_id)
            {
                agent.queue_len = agent.queue_len.saturating_sub(1);
            }
        }
    }
    state
        .agents
        .queued_claude_turns
        .retain(|turn| !clone_ids.contains(turn.agent_id.as_str()));

    for clone_id in clone_ids.iter() {
        state.agents.active_turns.remove(clone_id);
        state.agents.codex_thread_ids.remove(clone_id);
        state.agents.codex_used_tokens.remove(clone_id);
        state.agents.codex_context_remaining_pct.remove(clone_id);
        state
            .agents
            .codex_effective_context_window_tokens
            .remove(clone_id);
        state.agents.codex_default_reasoning_effort.remove(clone_id);
        state
            .agents
            .codex_supported_reasoning_efforts
            .remove(clone_id);
        state
            .agents
            .codex_selected_reasoning_effort
            .remove(clone_id);
        state.agents.claude_session_ids.remove(clone_id);
        state.agents.claude_used_tokens.remove(clone_id);
        state.agents.claude_context_remaining_pct.remove(clone_id);
        state
            .agents
            .claude_effective_context_window_tokens
            .remove(clone_id);
        state.agents.claude_default_effort.remove(clone_id);
        state.agents.claude_supported_efforts.remove(clone_id);
        state.agents.claude_selected_effort.remove(clone_id);
        state.agents.swarm_role_by_agent_id.remove(clone_id);
        state.agents.swarm_priority_agent_ids.remove(clone_id);
        state
            .agents
            .roster_tree_collapsed_agent_ids
            .remove(clone_id);
    }

    let mut remove_mission_thread_ids = false;
    if let Some(map) = state.agents.codex_mission_thread_ids.get_mut(mission_id) {
        map.retain(|agent_id, _| !clone_ids.contains(agent_id.as_str()));
        remove_mission_thread_ids = map.is_empty();
    }
    if remove_mission_thread_ids {
        state.agents.codex_mission_thread_ids.remove(mission_id);
    }

    let mut remove_mission_used_tokens = false;
    if let Some(map) = state.agents.codex_mission_used_tokens.get_mut(mission_id) {
        map.retain(|agent_id, _| !clone_ids.contains(agent_id.as_str()));
        remove_mission_used_tokens = map.is_empty();
    }
    if remove_mission_used_tokens {
        state.agents.codex_mission_used_tokens.remove(mission_id);
    }

    let mut remove_mission_context_remaining = false;
    if let Some(map) = state
        .agents
        .codex_mission_context_remaining_pct
        .get_mut(mission_id)
    {
        map.retain(|agent_id, _| !clone_ids.contains(agent_id.as_str()));
        remove_mission_context_remaining = map.is_empty();
    }
    if remove_mission_context_remaining {
        state
            .agents
            .codex_mission_context_remaining_pct
            .remove(mission_id);
    }

    let mut remove_claude_mission_session_ids = false;
    if let Some(map) = state.agents.claude_mission_session_ids.get_mut(mission_id) {
        map.retain(|agent_id, _| !clone_ids.contains(agent_id.as_str()));
        remove_claude_mission_session_ids = map.is_empty();
    }
    if remove_claude_mission_session_ids {
        state.agents.claude_mission_session_ids.remove(mission_id);
    }

    let mut remove_claude_mission_used_tokens = false;
    if let Some(map) = state.agents.claude_mission_used_tokens.get_mut(mission_id) {
        map.retain(|agent_id, _| !clone_ids.contains(agent_id.as_str()));
        remove_claude_mission_used_tokens = map.is_empty();
    }
    if remove_claude_mission_used_tokens {
        state.agents.claude_mission_used_tokens.remove(mission_id);
    }

    let mut remove_claude_mission_context_remaining = false;
    if let Some(map) = state
        .agents
        .claude_mission_context_remaining_pct
        .get_mut(mission_id)
    {
        map.retain(|agent_id, _| !clone_ids.contains(agent_id.as_str()));
        remove_claude_mission_context_remaining = map.is_empty();
    }
    if remove_claude_mission_context_remaining {
        state
            .agents
            .claude_mission_context_remaining_pct
            .remove(mission_id);
    }

    // Remove clones from the roster now that the mission is done.
    let selected_was_clone = state
        .agents
        .selected_agent
        .as_deref()
        .is_some_and(|id| clone_ids.contains(id));
    let base_of_selected = state
        .agents
        .selected_agent
        .as_deref()
        .and_then(swarm_clone_base_id)
        .map(str::to_string);

    state
        .agents
        .agents
        .retain(|lane| !clone_ids.contains(&lane.id));

    if selected_was_clone {
        if let Some(ref base_id) = base_of_selected {
            state.agents.selected_agent = Some(base_id.clone());
            state.agents.roster_selected = state
                .agents
                .agents
                .iter()
                .position(|lane| lane.id == *base_id)
                .unwrap_or(0);
        } else {
            state.agents.roster_selected = state
                .agents
                .roster_selected
                .min(state.agents.agents.len().saturating_sub(1));
            state.agents.selected_agent = state
                .agents
                .agents
                .get(state.agents.roster_selected)
                .map(|lane| lane.id.clone());
        }
        state.agents.roster_tree_selected = None;
    } else if let Some(selected_id) = state.agents.selected_agent.clone() {
        if let Some(idx) = state
            .agents
            .agents
            .iter()
            .position(|lane| lane.id == selected_id)
        {
            state.agents.roster_selected = idx;
        }
    }
}

/// Remove a single idle chat clone from the roster, preserving messages and artifacts.
pub fn cleanup_idle_chat_clone(state: &mut AppState, clone_id: &str) {
    if !is_chat_clone_agent_id(clone_id) {
        return;
    }
    // Only remove if actually idle with nothing queued.
    let dominated = state
        .agents
        .agents
        .iter()
        .any(|a| a.id == clone_id && a.status == AgentStatus::Idle && a.queue_len == 0);
    if !dominated {
        return;
    }

    // Purge runtime metadata (mirrors cleanup_swarm_clones_for_mission).
    state.agents.active_turns.remove(clone_id);
    state.agents.codex_thread_ids.remove(clone_id);
    state.agents.codex_used_tokens.remove(clone_id);
    state.agents.codex_context_remaining_pct.remove(clone_id);
    state
        .agents
        .codex_effective_context_window_tokens
        .remove(clone_id);
    state.agents.codex_default_reasoning_effort.remove(clone_id);
    state
        .agents
        .codex_supported_reasoning_efforts
        .remove(clone_id);
    state
        .agents
        .codex_selected_reasoning_effort
        .remove(clone_id);
    state.agents.swarm_role_by_agent_id.remove(clone_id);
    state.agents.swarm_priority_agent_ids.remove(clone_id);
    state
        .agents
        .roster_tree_collapsed_agent_ids
        .remove(clone_id);

    let selected_clone_removed = state.agents.selected_agent.as_deref() == Some(clone_id);
    let old_roster_selected = state.agents.roster_selected;

    state.agents.agents.retain(|lane| lane.id != clone_id);

    if state.agents.agents.is_empty() {
        state.agents.selected_agent = None;
        state.agents.roster_selected = 0;
        state.agents.roster_tree_selected = None;
        return;
    }

    if selected_clone_removed {
        state.agents.roster_selected =
            old_roster_selected.min(state.agents.agents.len().saturating_sub(1));
        state.agents.selected_agent = state
            .agents
            .agents
            .get(state.agents.roster_selected)
            .map(|lane| lane.id.clone());
        state.agents.roster_tree_selected = None;
    } else if let Some(selected_id) = state.agents.selected_agent.clone() {
        if let Some(idx) = state
            .agents
            .agents
            .iter()
            .position(|lane| lane.id == selected_id)
        {
            state.agents.roster_selected = idx;
        }
    }
}

pub fn create_chat_clone(state: &mut AppState, base_id: &str) -> Option<String> {
    let effective_base = chat_clone_base_id(base_id).unwrap_or(base_id);
    let base_lane = state
        .agents
        .agents
        .iter()
        .find(|lane| lane.id == effective_base)
        .cloned()?;

    let mut clone_num: usize = 0;
    loop {
        clone_num = clone_num.saturating_add(1);
        let candidate = format!("{effective_base}#chat-clone-{clone_num:02}");
        if state.agents.agents.iter().all(|lane| lane.id != candidate) {
            break;
        }
        if clone_num >= 99 {
            return None;
        }
    }

    let clone_id = format!("{effective_base}#chat-clone-{clone_num:02}");
    let base_role = base_lane.role.trim();
    let display_role = if base_role.is_empty() {
        format!("(clone {clone_num:02})")
    } else {
        format!("{base_role} (clone {clone_num:02})")
    };

    let mut lane = base_lane;
    lane.id = clone_id.clone();
    lane.role = display_role;
    lane.status = AgentStatus::Idle;
    lane.queue_len = 0;
    lane.heartbeat_age_secs = 0;
    lane.current_mission = None;

    insert_swarm_clone_lane(state, effective_base, lane);
    copy_codex_runtime_metadata(state, effective_base, &clone_id);
    copy_claude_runtime_metadata(state, effective_base, &clone_id);

    Some(clone_id)
}

pub(super) fn ensure_size_clones(
    state: &mut AppState,
    mission_id: &str,
    template: SwarmTemplate,
    size: SwarmSize,
    planner_agent_id: &str,
    agents: &mut Vec<String>,
) {
    if !matches!(
        template,
        SwarmTemplate::Lab | SwarmTemplate::Parallel | SwarmTemplate::Bulk
    ) {
        return;
    }
    if matches!(size, SwarmSize::All) {
        return;
    }

    let target = match size {
        SwarmSize::Default => DEFAULT_SWARM_SIZE,
        SwarmSize::All => MAX_SWARM_SIZE,
        SwarmSize::Count(n) => n,
    }
    .clamp(1, effective_max_swarm_size());
    if agents.len() >= target {
        return;
    }

    // Additional clones always come from the planner (the original
    // roster-selected agent), regardless of how many priority agents
    // were already picked.
    let Some(base_lane) = state
        .agents
        .agents
        .iter()
        .find(|lane| lane.id == planner_agent_id)
        .filter(|lane| lane.is_codex() || lane.is_claude())
        .cloned()
    else {
        return;
    };

    let mut clone_num: usize = 0;
    while agents.len() < target {
        clone_num = clone_num.saturating_add(1);
        let source_id = planner_agent_id;
        let clone_id = format!("{source_id}#swarm-{mission_id}-clone-{clone_num:02}");

        if agents.iter().any(|id| id == &clone_id) {
            continue;
        }

        if state.agents.agents.iter().all(|lane| lane.id != clone_id) {
            let mut lane = base_lane.clone();
            lane.id = clone_id.clone();
            let base_role = base_lane.role.trim();
            let display_role = if base_role.is_empty() {
                format!("(clone {clone_num:02})")
            } else {
                format!("{base_role} (clone {clone_num:02})")
            };
            lane.role = display_role;
            lane.status = AgentStatus::Idle;
            lane.heartbeat_age_secs = 0;
            lane.queue_len = 0;
            lane.current_mission = None;
            lane.last_message = String::new();
            insert_swarm_clone_lane(state, source_id, lane);
        }

        copy_codex_runtime_metadata(state, source_id, clone_id.as_str());
        copy_claude_runtime_metadata(state, source_id, clone_id.as_str());
        agents.push(clone_id);
    }
}
