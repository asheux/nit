use std::collections::HashMap;

use nit_core::{AgentMessage, AgentStatus, AppState, MissionPhase};

use super::{
    chat_clone_base_id, effective_max_swarm_size, is_chat_clone_agent_id, is_swarm_clone_agent_id,
    parse_swarm_template, swarm_clone_base_id, tasks_terminal_count, ParsedSwarmPlan,
    SwarmMissionKind, SwarmRun, SwarmSize, SwarmStage, SwarmTemplate, DEFAULT_SWARM_SIZE,
};

pub(super) fn next_mission_id(state: &AppState) -> String {
    format!("mis-{:03}", state.agents.missions.len() + 1)
}

pub(super) fn swarm_mission_title(
    root_prompt: &str,
    mission_id: &str,
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
) -> String {
    let first = root_prompt.lines().next().unwrap_or("Swarm mission").trim();
    let label = template.label();
    if first.is_empty() {
        return if matches!(mission_kind, SwarmMissionKind::General) {
            format!("{mission_id} swarm[{label}]")
        } else {
            format!("{mission_id} swarm[{label}] ({})", mission_kind.label())
        };
    }
    let mut title = String::new();
    for ch in first.chars().take(48) {
        title.push(ch);
    }
    if matches!(mission_kind, SwarmMissionKind::General) {
        format!("Swarm[{label}]: {title}")
    } else {
        format!("Swarm[{label}] ({}): {title}", mission_kind.label())
    }
}

pub(super) fn timestamp_label(state: &AppState) -> String {
    format!("t+{}", state.metrics.frame_count)
}

pub(super) fn update_mission_phase(state: &mut AppState, mission_id: &str, phase: MissionPhase) {
    let at = timestamp_label(state);
    if let Some(mission) = state
        .agents
        .missions
        .iter_mut()
        .find(|m| m.id == mission_id)
    {
        mission.phase = phase;
        mission.updated_at = at;
    }
}

pub(super) fn abort_swarm_plan_preflight(
    state: &mut AppState,
    run: &mut SwarmRun,
    parsed: ParsedSwarmPlan,
) {
    if parsed.integrator_agent_id.is_some() {
        run.integrator_agent_id = parsed.integrator_agent_id;
    }
    run.tasks = parsed.tasks;
    run.synthesis_prompt = parsed.synthesis_prompt;
    run.stage = SwarmStage::Planning;

    let at = timestamp_label(state);
    if let Some(mission) = state
        .agents
        .missions
        .iter_mut()
        .find(|mission| mission.id == run.mission_id)
    {
        mission.status = "FAILED".into();
        mission.phase = MissionPhase::Plan;
        mission.updated_at = at;
    }
}

pub(super) fn update_mission_final(state: &mut AppState, mission_id: &str, status: &str) {
    let at = timestamp_label(state);
    if let Some(mission) = state
        .agents
        .missions
        .iter_mut()
        .find(|m| m.id == mission_id)
    {
        mission.status = status.into();
        mission.phase = MissionPhase::Report;
        mission.updated_at = at;
    }
}

pub(super) fn update_mission_status(
    state: &mut AppState,
    run: &SwarmRun,
    done_override: Option<usize>,
) {
    let at = timestamp_label(state);
    let Some(mission) = state
        .agents
        .missions
        .iter_mut()
        .find(|mission| mission.id == run.mission_id)
    else {
        return;
    };

    let done = done_override.unwrap_or_else(|| tasks_terminal_count(&run.tasks));
    let total = run.tasks.len().max(1);
    let status = match run.stage {
        SwarmStage::Planning => "PLAN".into(),
        SwarmStage::Executing => format!("EXEC {done}/{total}"),
        SwarmStage::Verifying => "VERIFY".into(),
        SwarmStage::Synthesizing => "SYNTH".into(),
    };
    mission.status = status;
    mission.updated_at = at;
}

pub fn select_swarm_agents(
    state: &AppState,
    planner: &str,
    size: SwarmSize,
    template: Option<&str>,
) -> Vec<String> {
    let _template_kind = parse_swarm_template(template);
    let mut agents = vec![planner.to_string()];

    let roster_index = state
        .agents
        .agents
        .iter()
        .filter(|lane| {
            !is_swarm_clone_agent_id(lane.id.as_str()) && !is_chat_clone_agent_id(lane.id.as_str())
        })
        .enumerate()
        .map(|(idx, lane)| (lane.id.clone(), idx))
        .collect::<HashMap<_, _>>();

    let codex_pool = state
        .agents
        .agents
        .iter()
        .filter(|lane| lane.is_codex() || lane.is_claude())
        .filter(|lane| lane.id.as_str() != planner)
        .filter(|lane| {
            !is_swarm_clone_agent_id(lane.id.as_str()) && !is_chat_clone_agent_id(lane.id.as_str())
        })
        .map(|lane| lane.id.clone())
        .collect::<Vec<_>>();
    if codex_pool.is_empty() {
        return agents;
    }

    let target = match size {
        SwarmSize::Default => DEFAULT_SWARM_SIZE,
        SwarmSize::All => usize::MAX,
        SwarmSize::Count(n) => n,
    }
    .clamp(1, effective_max_swarm_size());
    let take = target.saturating_sub(1);
    if take == 0 {
        return agents;
    }

    #[derive(Clone)]
    struct Candidate {
        id: String,
        priority: bool,
        busy: bool,
        roster_idx: usize,
    }

    let mut pool: Vec<Candidate> = codex_pool
        .into_iter()
        .map(|id| Candidate {
            roster_idx: *roster_index.get(&id).unwrap_or(&usize::MAX),
            busy: is_agent_busy(state, id.as_str()),
            priority: is_priority_agent(state, id.as_str()),
            id,
        })
        .collect();

    let (mut priority_pool, _): (Vec<Candidate>, Vec<Candidate>) =
        pool.drain(..).partition(|candidate| candidate.priority);

    let mut selected: Vec<String> = Vec::new();

    if !priority_pool.is_empty() {
        priority_pool.sort_by(|a, b| {
            (a.busy as u8, a.roster_idx, &a.id).cmp(&(b.busy as u8, b.roster_idx, &b.id))
        });
        while selected.len() < take {
            let Some(candidate) = priority_pool.first().cloned() else {
                break;
            };
            priority_pool.remove(0);
            selected.push(candidate.id);
        }
    }

    agents.extend(selected);
    agents
}

/// What the operator would have gotten if the FD ceiling and roster pool
/// were both unbounded — i.e. the literal numeric request behind the
/// `SwarmSize` enum. Used by the `chat_input` flow to detect when an
/// explicit `@swarm N` got silently clamped to a smaller fan-out, so the
/// operator gets a "requested X, started Y" message instead of a confusing
/// reduced swarm.
///
/// For `All`: the count of non-clone codex/claude lanes currently in the
/// roster. For `Count(n)`: just `n`. For `Default`: `DEFAULT_SWARM_SIZE`.
pub fn swarm_intended_size(state: &AppState, size: SwarmSize) -> usize {
    match size {
        SwarmSize::Default => DEFAULT_SWARM_SIZE,
        SwarmSize::All => state
            .agents
            .agents
            .iter()
            .filter(|lane| lane.is_codex() || lane.is_claude())
            .filter(|lane| {
                !is_swarm_clone_agent_id(lane.id.as_str())
                    && !is_chat_clone_agent_id(lane.id.as_str())
            })
            .count()
            .max(1),
        SwarmSize::Count(n) => n.max(1),
    }
}

pub fn is_agent_busy(state: &AppState, agent_id: &str) -> bool {
    state.agents.active_turns.contains_key(agent_id)
        || state
            .agents
            .queued_codex_turns
            .iter()
            .any(|turn| turn.agent_id == agent_id)
        || state
            .agents
            .agents
            .iter()
            .find(|lane| lane.id.as_str() == agent_id)
            .is_some_and(|lane| matches!(lane.status, AgentStatus::Running))
}

/// Resolve any clone agent ID back to its base (non-clone) agent ID.
///
/// Understands chat clones (`#chat-clone-`), swarm clones (`#swarm-`), and
/// shadow clones (`#shadow-`). Without shadow-awareness here,
/// [`is_agent_family_busy`] wouldn't notice that a base agent's shadow
/// pipeline is in flight, and `@new` / queueing decisions would race.
pub fn resolve_base_agent_id(agent_id: &str) -> &str {
    chat_clone_base_id(agent_id)
        .or_else(|| swarm_clone_base_id(agent_id))
        .or_else(|| crate::shadow::parse_shadow_lane_id(agent_id).map(|(base, _, _)| base))
        .unwrap_or(agent_id)
}

pub fn is_agent_family_busy(state: &AppState, agent_id: &str) -> bool {
    let base = resolve_base_agent_id(agent_id);
    for lane in &state.agents.agents {
        if resolve_base_agent_id(&lane.id) != base {
            continue;
        }
        if state.agents.active_turns.contains_key(&lane.id)
            || matches!(lane.status, AgentStatus::Running)
        {
            return true;
        }
    }
    state
        .agents
        .queued_codex_turns
        .iter()
        .any(|turn| resolve_base_agent_id(&turn.agent_id) == base)
}

pub(super) fn is_priority_agent(state: &AppState, agent_id: &str) -> bool {
    if state.agents.swarm_priority_agent_ids.contains(agent_id) {
        return true;
    }
    if let Some(base_id) = swarm_clone_base_id(agent_id) {
        return state.agents.swarm_priority_agent_ids.contains(base_id);
    }
    false
}

pub fn push_system_message_to_mission(state: &mut AppState, mission_id: &str, text: String) {
    state.agents.messages.push(AgentMessage {
        at: timestamp_label(state),
        channel: nit_core::AgentChannel::Broadcast,
        agent_id: Some("swarm".into()),
        mission_id: Some(mission_id.to_string()),
        text,
        prompt_msg_idx: None,
        kind: None,
    });
}

/// Marker `kind` used by `push_system_alert_to_mission`. The chat console
/// hides ordinary `agent_id == "swarm"` broadcasts as redundant with
/// per-agent callouts; messages tagged `SYSTEM_ALERT_KIND` bypass that
/// filter so operator-facing warnings (FD-bound clamp, pool-bound clamp,
/// large-swarm advisory) always render.
pub const SYSTEM_ALERT_KIND: &str = "system-alert";

/// Like `push_system_message_to_mission` but tagged `SYSTEM_ALERT_KIND`
/// so the chat console renders it instead of filtering. Reserve for
/// alerts the operator must see (clamp warnings, ulimit advice).
pub fn push_system_alert_to_mission(state: &mut AppState, mission_id: &str, text: String) {
    state.agents.messages.push(AgentMessage {
        at: timestamp_label(state),
        channel: nit_core::AgentChannel::Broadcast,
        agent_id: Some("swarm".into()),
        mission_id: Some(mission_id.to_string()),
        text,
        prompt_msg_idx: None,
        kind: Some(SYSTEM_ALERT_KIND.into()),
    });
}

pub(super) fn tag_last_agent_message_kind(
    state: &mut AppState,
    agent_id: &str,
    mission_id: &str,
    kind: &str,
) {
    if let Some(msg) = state.agents.messages.iter_mut().rev().find(|msg| {
        msg.agent_id.as_deref() == Some(agent_id) && msg.mission_id.as_deref() == Some(mission_id)
    }) {
        msg.kind = Some(kind.to_string());
    }
}
