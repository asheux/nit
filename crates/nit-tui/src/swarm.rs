use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use nit_core::{AgentBusEvent, AgentMessage, AgentStatus, AppState, MissionPhase, MissionRecord};

const DEFAULT_SWARM_SIZE: usize = 4;
const MAX_SWARM_SIZE: usize = 16;
const SWARM_VERIFY_MAX_CHARS: usize = 12_000;
const SWARM_DEP_OUTPUT_MAX_CHARS: usize = 8_000;
/// Limit for roles that need full dependency output (judge, integrate, and any
/// write-role task).  Set high enough to avoid truncating detailed proposals —
/// a comprehensive multi-file refactoring proposal can reach 20-30K chars.
const SWARM_DEP_OUTPUT_MAX_CHARS_FULL: usize = 32_000;
const COMPUTATIONAL_RESEARCH_ROLE: &str = "computational-research";
const COMPUTATIONAL_RESEARCH_ROLE_LEGACY: &str = "computational research";

fn swarm_clone_base_id(agent_id: &str) -> Option<&str> {
    agent_id.split_once("#swarm-").map(|(base_id, _)| base_id)
}

fn is_swarm_clone_agent_id(agent_id: &str) -> bool {
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

fn is_swarm_clone_for_mission(agent_id: &str, mission_id: &str) -> bool {
    let Some((_base_id, rest)) = agent_id.split_once("#swarm-") else {
        return false;
    };
    rest.strip_prefix(mission_id)
        .is_some_and(|suffix| suffix.starts_with('-'))
}

fn copy_codex_runtime_metadata(state: &mut AppState, base_id: &str, clone_id: &str) {
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

fn copy_claude_runtime_metadata(state: &mut AppState, base_id: &str, clone_id: &str) {
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

fn insert_swarm_clone_lane(state: &mut AppState, base_id: &str, clone_lane: nit_core::AgentLane) {
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
fn drain_queued_turns_for_agent(state: &mut AppState, agent_id: &str) {
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

fn cleanup_swarm_clones_for_mission(state: &mut AppState, mission_id: &str) {
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

fn ensure_size_clones(
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
    .clamp(1, MAX_SWARM_SIZE);
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SwarmSize {
    Default,
    All,
    Count(usize),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SwarmTemplate {
    /// Parallel task splitting (v1-style): keep tasks independent and preferably one per agent.
    Parallel,
    /// "Lab" workflow: read-only analysis/proposal/review feeding a single-writer integrator.
    Lab,
    /// "Bulk orchestration": propose many candidate solutions in parallel, then converge via a
    /// judge step feeding a single-writer integrator.
    Bulk,
}

fn parse_swarm_template(value: Option<&str>) -> SwarmTemplate {
    let Some(value) = value else {
        return SwarmTemplate::Lab;
    };
    let value = value.trim();
    if value.eq_ignore_ascii_case("parallel") || value.eq_ignore_ascii_case("v1") {
        return SwarmTemplate::Parallel;
    }
    if value.eq_ignore_ascii_case("bulk") || value.eq_ignore_ascii_case("bo") {
        return SwarmTemplate::Bulk;
    }
    if value.eq_ignore_ascii_case("lab")
        || value.eq_ignore_ascii_case("default")
        || value.eq_ignore_ascii_case("v2")
    {
        return SwarmTemplate::Lab;
    }
    SwarmTemplate::Lab
}

impl SwarmTemplate {
    fn label(&self) -> &'static str {
        match self {
            SwarmTemplate::Parallel => "parallel",
            SwarmTemplate::Lab => "lab",
            SwarmTemplate::Bulk => "bulk",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SwarmMissionKind {
    General,
    Research,
    ComputationalResearch,
}

impl SwarmMissionKind {
    fn label(&self) -> &'static str {
        match self {
            SwarmMissionKind::General => "general",
            SwarmMissionKind::Research => "research",
            SwarmMissionKind::ComputationalResearch => COMPUTATIONAL_RESEARCH_ROLE,
        }
    }

    fn allows_research_roles(&self) -> bool {
        !matches!(self, SwarmMissionKind::General)
    }

    fn allows_role(&self, role: &str) -> bool {
        match normalize_role_label(role).as_deref() {
            Some("research") => matches!(
                self,
                SwarmMissionKind::Research | SwarmMissionKind::ComputationalResearch
            ),
            Some(COMPUTATIONAL_RESEARCH_ROLE) => {
                matches!(self, SwarmMissionKind::ComputationalResearch)
            }
            _ => true,
        }
    }
}

pub(crate) fn parse_swarm_mission_kind(value: Option<&str>) -> Option<SwarmMissionKind> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if value.eq_ignore_ascii_case("general")
        || value.eq_ignore_ascii_case("default")
        || value.eq_ignore_ascii_case("code")
        || value.eq_ignore_ascii_case("coding")
    {
        return Some(SwarmMissionKind::General);
    }
    if value.eq_ignore_ascii_case("research") {
        return Some(SwarmMissionKind::Research);
    }
    if value.eq_ignore_ascii_case("computational")
        || value.eq_ignore_ascii_case("computational-research")
        || value.eq_ignore_ascii_case("computational research")
        || value.eq_ignore_ascii_case("comp-research")
        || value.eq_ignore_ascii_case("comp_research")
    {
        return Some(SwarmMissionKind::ComputationalResearch);
    }
    None
}

pub(crate) fn explicit_swarm_mission_kind_from_prompt(
    root_prompt: &str,
) -> Option<SwarmMissionKind> {
    for line in root_prompt.lines() {
        let trimmed = line.trim().trim_start_matches(['-', '*', '•']).trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix("mission:") else {
            continue;
        };
        let value = rest.trim();
        if value.is_empty() {
            continue;
        }
        let value = value
            .trim_matches(|ch: char| matches!(ch, '`' | '"' | '\''))
            .trim();
        let token = value
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|ch: char| matches!(ch, ',' | '.' | ';' | ')'));
        if let Some(kind) = parse_swarm_mission_kind(Some(token)) {
            return Some(kind);
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwarmCommand {
    pub size: SwarmSize,
    pub template: Option<String>,
    pub mission_kind: Option<SwarmMissionKind>,
    pub prompt: String,
}

pub fn parse_swarm_command(raw: &str) -> Option<SwarmCommand> {
    let after = raw.trim_start().strip_prefix("@swarm")?;
    if after.is_empty() {
        return None;
    }
    if !after.starts_with(char::is_whitespace) {
        // Avoid treating "@swarmies" as a command.
        return None;
    }
    let mut rest = after.trim_start();
    if rest.is_empty() {
        return None;
    }

    let mut size = SwarmSize::Default;
    if let Some(next) = rest.split_whitespace().next() {
        if next.eq_ignore_ascii_case("all") {
            size = SwarmSize::All;
            rest = rest.strip_prefix(next).unwrap_or(rest).trim_start();
        } else if next.chars().all(|ch| ch.is_ascii_digit()) {
            if let Ok(n) = next.parse::<usize>() {
                size = SwarmSize::Count(n);
                rest = rest.strip_prefix(next).unwrap_or(rest).trim_start();
            }
        }
    }

    let mut template = None;
    let mut mission_kind = None;
    loop {
        let Some(next) = rest.split_whitespace().next() else {
            break;
        };
        if let Some(value) = next
            .strip_prefix("template=")
            .or_else(|| next.strip_prefix("t="))
        {
            let value = value.trim();
            if !value.is_empty() {
                template = Some(value.to_ascii_lowercase());
            }
            rest = rest.strip_prefix(next).unwrap_or(rest).trim_start();
            continue;
        }
        if let Some(value) = next
            .strip_prefix("mission=")
            .or_else(|| next.strip_prefix("m="))
        {
            mission_kind = parse_swarm_mission_kind(Some(value));
            rest = rest.strip_prefix(next).unwrap_or(rest).trim_start();
            continue;
        }
        break;
    }

    let prompt = rest.to_string();
    if prompt.trim().is_empty() {
        return None;
    }

    Some(SwarmCommand {
        size,
        template,
        mission_kind,
        prompt,
    })
}

#[derive(Clone, Debug)]
pub struct SwarmDispatch {
    pub agent_id: String,
    pub mission_id: String,
    pub prompt: String,
    /// Task role (e.g. "review", "code") to apply to the agent lane on dispatch.
    pub task_role: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SwarmArtifactFocus {
    Task { mission_id: String, task_id: String },
    Report { mission_id: String },
}

#[derive(Default)]
pub(crate) struct SwarmEventOutcome {
    pub dispatches: Vec<SwarmDispatch>,
    pub artifact_focus: Option<SwarmArtifactFocus>,
}

#[derive(Default)]
pub struct SwarmRuntime {
    runs: HashMap<String, SwarmRun>,
    completed_runs: HashMap<String, SwarmRun>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SwarmStage {
    Planning,
    Executing,
    Verifying,
    Synthesizing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum GateBundle {
    Rust,
    Node,
    Python,
    Go,
    Genome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Gate {
    name: &'static str,
    command: &'static str,
}

impl GateBundle {
    fn from_label(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("rust-ci") {
            return Some(Self::Rust);
        }
        if value.eq_ignore_ascii_case("node-ci") {
            return Some(Self::Node);
        }
        if value.eq_ignore_ascii_case("python-ci") {
            return Some(Self::Python);
        }
        if value.eq_ignore_ascii_case("go-ci") {
            return Some(Self::Go);
        }
        if value.eq_ignore_ascii_case("genome") || value.eq_ignore_ascii_case("genome-quality") {
            return Some(Self::Genome);
        }
        None
    }

    fn detect(state: &AppState) -> GateBundleSelection {
        let config_default = read_workspace_gate_default(state.workspace_root.as_path());
        if let Ok(Some(default)) = config_default.as_ref() {
            if default.eq_ignore_ascii_case("none") {
                return GateBundleSelection {
                    bundle: None,
                    source: "config:none".into(),
                };
            }
            if default.eq_ignore_ascii_case("auto") {
                // continue with auto-detection below
            } else if let Some(bundle) = Self::from_label(default) {
                return GateBundleSelection {
                    bundle: Some(bundle.clone()),
                    source: format!("config:{}", bundle.label()),
                };
            }
        }

        let mut detected = None;
        let mut cursor = Some(state.workspace_root.as_path());
        while let Some(path) = cursor {
            if path.join("Cargo.toml").exists() {
                detected = Some((Self::Rust, "Cargo.toml"));
                break;
            }
            if path.join("package.json").exists() {
                detected = Some((Self::Node, "package.json"));
                break;
            }
            if path.join("pyproject.toml").exists() {
                detected = Some((Self::Python, "pyproject.toml"));
                break;
            }
            if path.join("requirements.txt").exists() {
                detected = Some((Self::Python, "requirements.txt"));
                break;
            }
            if path.join("setup.cfg").exists() {
                detected = Some((Self::Python, "setup.cfg"));
                break;
            }
            if path.join("setup.py").exists() {
                detected = Some((Self::Python, "setup.py"));
                break;
            }
            if path.join("go.mod").exists() {
                detected = Some((Self::Go, "go.mod"));
                break;
            }
            cursor = path.parent();
        }

        let parse_error = config_default
            .err()
            .map(|err| format!("config-error:{err}"));
        if let Some((bundle, marker)) = detected {
            return GateBundleSelection {
                bundle: Some(bundle.clone()),
                source: parse_error
                    .map(|prefix| format!("{prefix}|auto:{}({marker})", bundle.label()))
                    .unwrap_or_else(|| format!("auto:{}({marker})", bundle.label())),
            };
        }

        GateBundleSelection {
            bundle: None,
            source: parse_error.unwrap_or_else(|| "auto:none".into()),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            GateBundle::Rust => "rust-ci",
            GateBundle::Node => "node-ci",
            GateBundle::Python => "python-ci",
            GateBundle::Go => "go-ci",
            GateBundle::Genome => "genome",
        }
    }

    fn gates(&self) -> Vec<Gate> {
        match self {
            GateBundle::Rust => vec![
                Gate {
                    name: "fmt",
                    command: "cargo fmt --all -- --check",
                },
                Gate {
                    name: "clippy",
                    command: "cargo clippy --all-targets --all-features -- -D warnings",
                },
                Gate {
                    name: "test",
                    command: "cargo test --workspace --all-features",
                },
            ],
            GateBundle::Node => vec![
                Gate {
                    name: "lint",
                    command: "npm run lint --if-present",
                },
                Gate {
                    name: "build",
                    command: "npm run build --if-present",
                },
                Gate {
                    name: "test",
                    command: "npm test -- --watch=false --passWithNoTests",
                },
            ],
            GateBundle::Python => vec![
                Gate {
                    name: "ruff",
                    command: "python -m ruff check .",
                },
                Gate {
                    name: "mypy",
                    command: "python -m mypy .",
                },
                Gate {
                    name: "pytest",
                    command: "python -m pytest -q",
                },
            ],
            GateBundle::Go => vec![
                Gate {
                    name: "fmt",
                    command: "gofmt -l .",
                },
                Gate {
                    name: "vet",
                    command: "go vet ./...",
                },
                Gate {
                    name: "test",
                    command: "go test ./...",
                },
            ],
            GateBundle::Genome => vec![Gate {
                name: "genome-quality",
                command: "(evaluated locally by nit)",
            }],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GateBundleSelection {
    bundle: Option<GateBundle>,
    source: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GateReport {
    pub overall_ok: bool,
    pub gates: Vec<GateReportGate>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GateReportGate {
    pub name: String,
    pub command: String,
    pub ok: bool,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

impl GateReportGate {
    fn ui_status(&self) -> &'static str {
        if let Some(status) = self.status.as_deref() {
            if status.eq_ignore_ascii_case("pass")
                || status.eq_ignore_ascii_case("ok")
                || status.eq_ignore_ascii_case("success")
            {
                return "PASS";
            }
            if status.eq_ignore_ascii_case("skip") || status.eq_ignore_ascii_case("skipped") {
                return "SKIP";
            }
            if status.eq_ignore_ascii_case("fail") || status.eq_ignore_ascii_case("failed") {
                return "FAIL";
            }
        }
        if self.ok {
            "PASS"
        } else {
            "FAIL"
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SwarmTaskState {
    Pending,
    Ready,
    Dispatched,
    Running,
    Done,
    Failed,
    Skipped,
}

impl SwarmTaskState {
    fn is_terminal(self) -> bool {
        matches!(
            self,
            SwarmTaskState::Done | SwarmTaskState::Failed | SwarmTaskState::Skipped
        )
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SwarmDagValidationMode {
    /// Reject plans with cycles/unknown deps (do not auto-repair).
    Strict,
    /// Attempt to make the graph runnable (drop unknown deps + break cycles) with warnings.
    Repair,
}

const DEFAULT_DAG_VALIDATION_MODE: SwarmDagValidationMode = SwarmDagValidationMode::Strict;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SwarmTaskArtifacts {
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub files: Vec<SwarmArtifactFile>,
    #[serde(default)]
    pub diffs: Vec<SwarmArtifactDiff>,
    #[serde(default)]
    pub commands: Vec<SwarmArtifactCommand>,
    #[serde(default)]
    pub risks: Vec<SwarmArtifactRisk>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl SwarmTaskArtifacts {
    fn is_empty(&self) -> bool {
        self.summary
            .as_deref()
            .is_none_or(|summary| summary.trim().is_empty())
            && self.files.is_empty()
            && self.diffs.is_empty()
            && self.commands.is_empty()
            && self.risks.is_empty()
            && self.notes.is_empty()
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SwarmArtifactFile {
    pub path: String,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SwarmArtifactDiff {
    #[serde(default)]
    pub path: Option<String>,
    pub summary: String,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SwarmArtifactCommand {
    pub cmd: String,
    #[serde(default)]
    pub purpose: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SwarmArtifactRisk {
    #[serde(default)]
    pub level: Option<String>,
    pub item: String,
    #[serde(default)]
    pub mitigation: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SwarmTaskDashboardRow {
    pub id: String,
    pub title: String,
    pub role: Option<String>,
    pub agent_id: String,
    pub state: String,
    pub deps: Vec<String>,
    pub blocked_on: Vec<String>,
    pub writes: bool,
    pub done_when: Option<String>,
    pub output_present: bool,
}

#[derive(Clone, Debug)]
pub struct SwarmGateDashboardRow {
    pub name: String,
    pub command: String,
    pub status: String,
    pub notes: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SwarmDashboardView {
    pub mission_id: String,
    pub template: String,
    pub phase: String,
    pub done: usize,
    pub failed: usize,
    pub skipped: usize,
    pub running: usize,
    pub queued: usize,
    pub pending: usize,
    pub tasks: Vec<SwarmTaskDashboardRow>,
    pub gate_bundle: Option<String>,
    pub gates: Vec<SwarmGateDashboardRow>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct SwarmTaskPersistenceView {
    pub id: String,
    pub title: String,
    pub role: Option<String>,
    pub agent_id: String,
    pub state: String,
    pub deps: Vec<String>,
    pub blocked_on: Vec<String>,
    pub writes: bool,
    pub done_when: Option<String>,
    pub expected_artifacts: Vec<String>,
    pub expected_artifacts_missing: bool,
    pub output_present: bool,
    pub output: Option<String>,
    pub artifacts: Option<SwarmTaskArtifacts>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct SwarmPersistenceView {
    pub mission_id: String,
    pub template: String,
    pub phase: String,
    pub gate_bundle: Option<String>,
    pub gate_selection: String,
    pub gate_report: Option<GateReport>,
    pub gate_output: Option<String>,
    pub report_status: Option<String>,
    pub report_agent_id: Option<String>,
    pub report_output: Option<String>,
    pub tasks: Vec<SwarmTaskPersistenceView>,
}

#[derive(Clone, Debug)]
struct SwarmTask {
    id: String,
    agent_id: String,
    role: Option<String>,
    title: String,
    task_prompt: String,
    deps: Vec<String>,
    writes: bool,
    artifacts: Vec<String>,
    done_when: Option<String>,
    state: SwarmTaskState,
    output: Option<String>,
    parsed_artifacts: Option<SwarmTaskArtifacts>,
    expected_artifacts_missing: bool,
    failed: bool,
    /// Number of times this task has been retried after failure.
    retries: u8,
}

#[derive(Clone, Debug)]
struct SwarmRun {
    mission_id: String,
    root_prompt: String,
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    planner_agent_id: String,
    integrator_agent_id: Option<String>,
    integrator_locked: bool,
    verifier_agent_id: Option<String>,
    gate_bundle: Option<GateBundle>,
    gate_selection: String,
    agent_ids: Vec<String>,
    stage: SwarmStage,
    tasks: Vec<SwarmTask>,
    synthesis_prompt: Option<String>,
    gate_output: Option<String>,
    gate_report: Option<GateReport>,
    genome_gate_results: Option<String>,
    report_status: Option<String>,
    report_output: Option<String>,
    /// Source files in the scope referenced by the operator prompt (e.g.
    /// `crates/nit-games`).  Populated at run creation; injected into
    /// integrate task prompts so agents cannot skip files.
    scope_files: Vec<String>,
}

/// Configuration from a previous swarm run, used to re-launch follow-up prompts
/// with the same template, size, and planner.
pub struct SwarmSessionConfig {
    pub template: String,
    pub size: usize,
    pub planner_agent_id: String,
}

/// Re-create swarm clones for a follow-up dispatch within an existing mission.
/// Returns the full list of agent IDs (planner + clones) ready for dispatch.
pub fn ensure_swarm_agents_for_followup(
    state: &mut AppState,
    mission_id: &str,
    config: &SwarmSessionConfig,
) -> Vec<String> {
    let template = parse_swarm_template(Some(config.template.as_str()));
    let size = SwarmSize::Count(config.size);
    let mut agents = vec![config.planner_agent_id.clone()];
    ensure_size_clones(
        state,
        mission_id,
        template,
        size,
        &config.planner_agent_id,
        &mut agents,
    );
    // Update the mission's assigned_agents so broadcast_target_agents can find them.
    if let Some(mission) = state
        .agents
        .missions
        .iter_mut()
        .find(|m| m.id == mission_id)
    {
        mission.assigned_agents = agents.clone();
    }
    agents
}

impl SwarmRuntime {
    pub fn is_active_mission(&self, mission_id: &str) -> bool {
        self.runs.contains_key(mission_id)
    }

    /// Returns the current swarm stage label (e.g. "VERIFY", "SYNTH") for a mission.
    pub fn swarm_stage_label(&self, mission_id: &str) -> Option<&'static str> {
        self.run_for_mission(mission_id)
            .map(|run| stage_label(run.stage))
    }

    /// Returns the swarm configuration for a mission (active or completed)
    /// so follow-up prompts can reuse the same template and agent count.
    pub fn session_config(&self, mission_id: &str) -> Option<SwarmSessionConfig> {
        let run = self.run_for_mission(mission_id)?;
        Some(SwarmSessionConfig {
            template: run.template.label().to_string(),
            size: run.agent_ids.len(),
            planner_agent_id: run.planner_agent_id.clone(),
        })
    }

    /// Build a planner prompt for a follow-up, wrapping the user's raw text
    /// with the same planning instructions used for the initial `@swarm`.
    pub fn build_followup_planner_prompt(
        &self,
        state: &AppState,
        mission_id: &str,
        user_prompt: &str,
    ) -> Option<String> {
        let run = self.run_for_mission(mission_id)?;
        let mut role_hints: Vec<(String, String)> = Vec::new();
        if matches!(run.template, SwarmTemplate::Parallel | SwarmTemplate::Bulk) {
            for id in run
                .agent_ids
                .iter()
                .filter(|id| id.as_str() != run.planner_agent_id.as_str())
            {
                role_hints.push((
                    id.clone(),
                    planner_role_hint_for_agent(
                        &state.agents.swarm_role_by_agent_id,
                        id.as_str(),
                        run.integrator_agent_id.as_deref(),
                        run.mission_kind,
                    ),
                ));
            }
            deduplicate_inherited_role_hints(&mut role_hints, &state.agents.swarm_role_by_agent_id);
        }
        let mut priority_agent_ids: Vec<String> = Vec::new();
        if matches!(run.template, SwarmTemplate::Parallel | SwarmTemplate::Bulk) {
            for id in run
                .agent_ids
                .iter()
                .filter(|id| id.as_str() != run.planner_agent_id.as_str())
            {
                if is_priority_agent(state, id.as_str()) {
                    priority_agent_ids.push(id.clone());
                }
            }
        }
        Some(build_planner_prompt(
            user_prompt,
            run.template,
            run.mission_kind,
            &run.planner_agent_id,
            &run.agent_ids,
            run.integrator_agent_id.as_deref(),
            &role_hints,
            &priority_agent_ids,
            state.workspace_root.as_path(),
        ))
    }

    /// Re-activate a completed swarm run so the planner can generate a new
    /// plan for a follow-up prompt.  Clears previous tasks/outputs while
    /// keeping agent assignments and gate config intact.
    pub fn reactivate_for_followup(&mut self, state: &mut AppState, mission_id: &str) -> bool {
        let Some(mut run) = self.completed_runs.remove(mission_id) else {
            // Already active or doesn't exist.
            return self.runs.contains_key(mission_id);
        };

        // Push the swarm meta message so the footer shows Mission/Gates.
        push_system_message_to_mission(
            state,
            mission_id,
            format!(
                "Swarm template: {} | mission: {} | integrator: {} | verifier: {} | gates: {}",
                run.template.label(),
                run.mission_kind.label(),
                run.integrator_agent_id.as_deref().unwrap_or("(none)"),
                run.verifier_agent_id.as_deref().unwrap_or("(none)"),
                gate_bundle_label(run.gate_bundle.as_ref(), &run.gate_selection),
            ),
        );

        run.stage = SwarmStage::Planning;
        run.tasks.clear();
        run.synthesis_prompt = None;
        run.gate_output = None;
        run.gate_report = None;
        run.report_status = None;
        run.report_output = None;
        self.runs.insert(mission_id.to_string(), run);
        true
    }

    pub fn swarm_dashboard(&self, mission_id: &str) -> Option<SwarmDashboardView> {
        let run = self.run_for_mission(mission_id)?;
        let tasks = run
            .tasks
            .iter()
            .map(|task| SwarmTaskDashboardRow {
                id: task.id.clone(),
                title: task.title.clone(),
                role: task.role.clone(),
                agent_id: task.agent_id.clone(),
                state: task_state_dashboard_label(task.state).into(),
                deps: task.deps.clone(),
                blocked_on: blocked_on(run, task),
                writes: task.writes,
                done_when: task.done_when.clone(),
                output_present: task.output.is_some(),
            })
            .collect::<Vec<_>>();

        let mut done = 0usize;
        let mut failed = 0usize;
        let mut skipped = 0usize;
        let mut running = 0usize;
        let mut queued = 0usize;
        let mut pending = 0usize;
        for task in run.tasks.iter() {
            match task.state {
                SwarmTaskState::Done => done += 1,
                SwarmTaskState::Failed => failed += 1,
                SwarmTaskState::Skipped => skipped += 1,
                SwarmTaskState::Running => running += 1,
                SwarmTaskState::Ready | SwarmTaskState::Dispatched => queued += 1,
                SwarmTaskState::Pending => pending += 1,
            }
        }

        Some(SwarmDashboardView {
            mission_id: run.mission_id.clone(),
            template: run.template.label().into(),
            phase: stage_label(run.stage).into(),
            done,
            failed,
            skipped,
            running,
            queued,
            pending,
            tasks,
            gate_bundle: run
                .gate_bundle
                .as_ref()
                .map(|bundle| bundle.label().to_string()),
            gates: dashboard_gate_rows(run),
        })
    }

    pub fn swarm_persistence(&self, mission_id: &str) -> Option<SwarmPersistenceView> {
        let run = self.run_for_mission(mission_id)?;
        let tasks = run
            .tasks
            .iter()
            .map(|task| SwarmTaskPersistenceView {
                id: task.id.clone(),
                title: task.title.clone(),
                role: task.role.clone(),
                agent_id: task.agent_id.clone(),
                state: task_state_dashboard_label(task.state).into(),
                deps: task.deps.clone(),
                blocked_on: blocked_on(run, task),
                writes: task.writes,
                done_when: task.done_when.clone(),
                expected_artifacts: task.artifacts.clone(),
                expected_artifacts_missing: task.expected_artifacts_missing,
                output_present: task.output.is_some(),
                output: task.output.clone(),
                artifacts: task.parsed_artifacts.clone(),
            })
            .collect::<Vec<_>>();
        Some(SwarmPersistenceView {
            mission_id: run.mission_id.clone(),
            template: run.template.label().into(),
            phase: stage_label(run.stage).into(),
            gate_bundle: run
                .gate_bundle
                .as_ref()
                .map(|bundle| bundle.label().to_string()),
            gate_selection: run.gate_selection.clone(),
            gate_report: run.gate_report.clone(),
            gate_output: run.gate_output.clone(),
            report_status: run.report_status.clone(),
            report_agent_id: run
                .report_output
                .as_ref()
                .map(|_| run.planner_agent_id.clone()),
            report_output: run.report_output.clone(),
            tasks,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &mut self,
        state: &mut AppState,
        planner_agent_id: String,
        agent_ids: Vec<String>,
        size: SwarmSize,
        template: Option<String>,
        mission_kind: Option<SwarmMissionKind>,
        root_prompt: String,
    ) -> Option<(String, Vec<SwarmDispatch>)> {
        let mut agents = Vec::new();
        for agent_id in agent_ids {
            if agents.iter().any(|id: &String| id == &agent_id) {
                continue;
            }
            let is_codex = state
                .agents
                .agents
                .iter()
                .find(|lane| lane.id == agent_id)
                .is_some_and(|lane| lane.is_codex() || lane.is_claude());
            if is_codex {
                agents.push(agent_id);
            }
        }
        if !agents.iter().any(|id| id == &planner_agent_id) {
            agents.insert(0, planner_agent_id.clone());
        }

        let template_kind = parse_swarm_template(template.as_deref());
        let mission_kind = classify_swarm_mission_kind(&root_prompt, mission_kind);
        let mission_id = next_mission_id(state);

        let restore_roster_selected = state
            .agents
            .agents
            .get(state.agents.roster_selected)
            .map(|lane| lane.id.clone());

        ensure_size_clones(
            state,
            &mission_id,
            template_kind,
            size,
            planner_agent_id.as_str(),
            &mut agents,
        );

        if let Some(selected_id) = restore_roster_selected {
            if let Some(idx) = state
                .agents
                .agents
                .iter()
                .position(|lane| lane.id == selected_id)
            {
                state.agents.roster_selected = idx;
            }
        }
        if agents.len() < 2 {
            return None;
        }

        let title = swarm_mission_title(&root_prompt, &mission_id, template_kind, mission_kind);
        let at = timestamp_label(state);
        state.agents.missions.push(MissionRecord {
            id: mission_id.clone(),
            title,
            phase: MissionPhase::Plan,
            swarm: true,
            assigned_agents: agents.clone(),
            status: "PLAN".into(),
            updated_at: at.clone(),
        });
        state.agents.mission_selected = state.agents.missions.len().saturating_sub(1);
        state.agents.selected_mission = Some(mission_id.clone());

        state.agents.alerts.push(nit_core::AgentAlert {
            severity: nit_core::AgentAlertSeverity::Info,
            source: "swarm".into(),
            message: format!(
                "Created swarm mission {mission_id} with agents: {}",
                agents.join(", ")
            ),
            at,
        });

        let mut integrator_locked = false;
        let mut integrator_agent_id: Option<String> = None;
        if matches!(template_kind, SwarmTemplate::Parallel) {
            integrator_agent_id = agents
                .iter()
                .filter(|id| id.as_str() != planner_agent_id.as_str())
                .find(|id| {
                    direct_role_hint_for_agent(&state.agents.swarm_role_by_agent_id, id.as_str())
                        .as_deref()
                        == Some("integrate")
                })
                .cloned();
        }
        if matches!(template_kind, SwarmTemplate::Lab | SwarmTemplate::Bulk)
            || (matches!(template_kind, SwarmTemplate::Parallel) && integrator_agent_id.is_none())
        {
            let eligible = agents
                .iter()
                .filter(|id| id.as_str() != planner_agent_id.as_str())
                .cloned()
                .collect::<Vec<_>>();
            let priority_eligible = eligible
                .iter()
                .filter(|id| is_priority_agent(state, id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            let candidates = if !priority_eligible.is_empty() {
                priority_eligible
            } else {
                eligible
            };

            integrator_agent_id = candidates.first().cloned();

            if matches!(template_kind, SwarmTemplate::Bulk) {
                if let Some(integrator) = candidates
                    .iter()
                    .find(|id| {
                        state
                            .agents
                            .swarm_role_by_agent_id
                            .get(*id)
                            .map(|role| role.trim())
                            .filter(|role| !role.is_empty())
                            .is_some_and(|role| role.eq_ignore_ascii_case("integrate"))
                    })
                    .cloned()
                {
                    integrator_agent_id = Some(integrator);
                    integrator_locked = true;
                }
            }
        }
        let mut role_hints: Vec<(String, String)> = Vec::new();
        if matches!(template_kind, SwarmTemplate::Parallel | SwarmTemplate::Bulk) {
            for id in agents
                .iter()
                .filter(|id| id.as_str() != planner_agent_id.as_str())
            {
                role_hints.push((
                    id.clone(),
                    planner_role_hint_for_agent(
                        &state.agents.swarm_role_by_agent_id,
                        id.as_str(),
                        integrator_agent_id.as_deref(),
                        mission_kind,
                    ),
                ));
            }
            deduplicate_inherited_role_hints(&mut role_hints, &state.agents.swarm_role_by_agent_id);
        }
        let mut priority_agent_ids: Vec<String> = Vec::new();
        if matches!(template_kind, SwarmTemplate::Parallel | SwarmTemplate::Bulk) {
            for id in agents
                .iter()
                .filter(|id| id.as_str() != planner_agent_id.as_str())
            {
                if is_priority_agent(state, id.as_str()) {
                    priority_agent_ids.push(id.clone());
                }
            }
        }

        let plan_prompt = build_planner_prompt(
            &root_prompt,
            template_kind,
            mission_kind,
            &planner_agent_id,
            &agents,
            integrator_agent_id.as_deref(),
            &role_hints,
            &priority_agent_ids,
            state.workspace_root.as_path(),
        );

        let scope_files = enumerate_scope_files(state.workspace_root.as_path(), &root_prompt);

        let gate_selection = GateBundle::detect(state);
        let gate_bundle = gate_selection.bundle.clone();
        let verifier_agent_id = gate_bundle.as_ref().and_then(|_| {
            let eligible = agents
                .iter()
                .filter(|id| id.as_str() != planner_agent_id.as_str())
                .cloned()
                .collect::<Vec<_>>();
            let priority_eligible = eligible
                .iter()
                .filter(|id| is_priority_agent(state, id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            let candidates = if !priority_eligible.is_empty() {
                priority_eligible
            } else {
                eligible
            };

            if let Some(integrator) = integrator_agent_id.as_deref() {
                candidates
                    .iter()
                    .find(|id| id.as_str() != integrator)
                    .cloned()
                    .or_else(|| candidates.first().cloned())
            } else {
                candidates.first().cloned()
            }
        });

        push_system_message_to_mission(
            state,
            &mission_id,
            format!(
                "Swarm template: {} | mission: {} | integrator: {} | verifier: {} | gates: {}",
                template_kind.label(),
                mission_kind.label(),
                integrator_agent_id.as_deref().unwrap_or("(none)"),
                verifier_agent_id.as_deref().unwrap_or("(none)"),
                gate_bundle_label(gate_bundle.as_ref(), &gate_selection.source)
            ),
        );

        self.completed_runs.remove(&mission_id);
        self.runs.insert(
            mission_id.clone(),
            SwarmRun {
                mission_id: mission_id.clone(),
                root_prompt,
                template: template_kind,
                mission_kind,
                planner_agent_id: planner_agent_id.clone(),
                integrator_agent_id: integrator_agent_id.clone(),
                integrator_locked,
                verifier_agent_id,
                gate_bundle,
                gate_selection: gate_selection.source,
                agent_ids: agents,
                stage: SwarmStage::Planning,
                tasks: Vec::new(),
                synthesis_prompt: None,
                gate_output: None,
                gate_report: None,
                genome_gate_results: None,
                report_status: None,
                report_output: None,
                scope_files,
            },
        );

        Some((
            mission_id.clone(),
            vec![SwarmDispatch {
                agent_id: planner_agent_id,
                mission_id,
                prompt: plan_prompt,
                task_role: None,
            }],
        ))
    }

    pub fn handle_event(
        &mut self,
        state: &mut AppState,
        event: &AgentBusEvent,
    ) -> Vec<SwarmDispatch> {
        self.handle_event_outcome(state, event).dispatches
    }

    pub(crate) fn handle_event_outcome(
        &mut self,
        state: &mut AppState,
        event: &AgentBusEvent,
    ) -> SwarmEventOutcome {
        let mut outcome = SwarmEventOutcome::default();

        // Chat clones are ad-hoc agents; they must never interact with swarm runs.
        let event_agent_id = match event {
            AgentBusEvent::TurnStarted { agent_id, .. }
            | AgentBusEvent::TurnCompleted { agent_id, .. }
            | AgentBusEvent::TurnFailed { agent_id, .. } => Some(agent_id.as_str()),
            _ => None,
        };
        if event_agent_id.is_some_and(is_chat_clone_agent_id) {
            return outcome;
        }

        let dispatches = &mut outcome.dispatches;

        match event {
            AgentBusEvent::TurnStarted {
                agent_id,
                mission_id: Some(mid),
                ..
            } => {
                if let Some(run) = self.runs.get_mut(mid) {
                    mark_task_running(run, agent_id);
                    update_mission_status(state, run, None);
                }
            }
            AgentBusEvent::TurnCompleted {
                agent_id,
                mission_id: Some(mid),
                message,
                ..
            } => {
                let Some(mut run) = self.runs.remove(mid) else {
                    return outcome;
                };
                match run.stage {
                    SwarmStage::Planning if agent_id == &run.planner_agent_id => {
                        tag_last_agent_message_kind(state, agent_id, &run.mission_id, "plan");
                        let available = run
                            .agent_ids
                            .iter()
                            .filter(|id| *id != &run.planner_agent_id)
                            .cloned()
                            .collect::<Vec<_>>();
                        let multi_integrator = run.scope_files.len() > 15;
                        let mut parsed = parse_plan_from_planner(
                            message,
                            run.template,
                            run.mission_kind,
                            &run.root_prompt,
                            &available,
                            run.integrator_agent_id.as_deref(),
                            run.integrator_locked,
                            multi_integrator,
                        );
                        parsed.warnings.extend(apply_role_dependency_ordering(
                            state.workspace_root.as_path(),
                            &state.agents.swarm_role_by_agent_id,
                            run.mission_kind,
                            run.integrator_agent_id.as_deref(),
                            parsed.tasks.as_mut_slice(),
                            multi_integrator,
                        ));
                        parsed.warnings.extend(ensure_integrate_task(
                            &mut parsed.tasks,
                            run.mission_kind,
                            run.integrator_agent_id
                                .as_deref()
                                .or(parsed.integrator_agent_id.as_deref()),
                        ));

                        let dag_mode = match read_workspace_dag_validation_mode(
                            state.workspace_root.as_path(),
                        ) {
                            Ok(Some(mode)) => mode,
                            Ok(None) => DEFAULT_DAG_VALIDATION_MODE,
                            Err(err) => {
                                push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    format!(
                                        "PLAN warning: DAG validation config error: {err}; using default mode 'strict'."
                                    ),
                                );
                                DEFAULT_DAG_VALIDATION_MODE
                            }
                        };

                        let mut abort_execution = false;
                        let dag_issues = analyze_swarm_dag(parsed.tasks.as_slice());
                        if !dag_issues.is_empty() {
                            match dag_mode {
                                SwarmDagValidationMode::Strict => {
                                    push_system_message_to_mission(
                                        state,
                                        &run.mission_id,
                                        format!(
                                            "PLAN error: invalid task DAG ({}). Set `[swarm] dag_validation = \"repair\"` in `.nit/config.toml` to auto-repair.",
                                            dag_issues.summary()
                                        ),
                                    );
                                    for task in parsed.tasks.iter_mut() {
                                        if task.state.is_terminal() {
                                            continue;
                                        }
                                        task.state = SwarmTaskState::Skipped;
                                        task.failed = true;
                                        if task.output.is_none() {
                                            task.output = Some(
                                                "SKIPPED (preflight: invalid task DAG)".into(),
                                            );
                                        }
                                    }
                                    abort_execution = true;
                                }
                                SwarmDagValidationMode::Repair => {
                                    let mut warnings =
                                        repair_swarm_dag(parsed.tasks.as_mut_slice());
                                    let after = analyze_swarm_dag(parsed.tasks.as_slice());
                                    if !after.is_empty() {
                                        push_system_message_to_mission(
                                            state,
                                            &run.mission_id,
                                            format!(
                                                "PLAN error: DAG auto-repair failed ({}); aborting.",
                                                after.summary()
                                            ),
                                        );
                                        for task in parsed.tasks.iter_mut() {
                                            if task.state.is_terminal() {
                                                continue;
                                            }
                                            task.state = SwarmTaskState::Skipped;
                                            task.failed = true;
                                            if task.output.is_none() {
                                                task.output = Some(
                                                    "SKIPPED (preflight: DAG auto-repair failed)"
                                                        .into(),
                                                );
                                            }
                                        }
                                        abort_execution = true;
                                    } else if warnings.is_empty() {
                                        warnings.push(
                                            "DAG repair: plan had DAG issues; no changes needed."
                                                .into(),
                                        );
                                    }
                                    parsed.warnings.append(&mut warnings);
                                }
                            }
                        }

                        for warning in parsed.warnings.iter() {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                format!("PLAN warning: {warning}"),
                            );
                        }
                        if abort_execution {
                            abort_swarm_plan_preflight(state, &mut run, parsed);
                            cleanup_swarm_clones_for_mission(state, &run.mission_id);
                            self.completed_runs.insert(mid.clone(), run);
                            return outcome;
                        }
                        let prev_integrator = run.integrator_agent_id.clone();
                        let prev_verifier = run.verifier_agent_id.clone();
                        if parsed.integrator_agent_id.is_some() {
                            run.integrator_agent_id = parsed.integrator_agent_id.clone();
                        }
                        if run.gate_bundle.is_some() {
                            run.verifier_agent_id = {
                                if let Some(integrator) = run.integrator_agent_id.as_deref() {
                                    run.agent_ids
                                        .iter()
                                        .find(|id| {
                                            id.as_str() != run.planner_agent_id.as_str()
                                                && id.as_str() != integrator
                                        })
                                        .cloned()
                                        .or_else(|| {
                                            run.agent_ids
                                                .iter()
                                                .find(|id| {
                                                    id.as_str() != run.planner_agent_id.as_str()
                                                })
                                                .cloned()
                                        })
                                } else {
                                    run.agent_ids
                                        .iter()
                                        .find(|id| id.as_str() != run.planner_agent_id.as_str())
                                        .cloned()
                                }
                            };
                        }
                        if prev_integrator != run.integrator_agent_id
                            || prev_verifier != run.verifier_agent_id
                        {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                format!(
                                    "Swarm template: {} | integrator: {} | verifier: {} | gates: {}",
                                    run.template.label(),
                                    run.integrator_agent_id
                                        .as_deref()
                                        .unwrap_or("(none)"),
                                    run.verifier_agent_id.as_deref().unwrap_or("(none)"),
                                    gate_bundle_label(
                                        run.gate_bundle.as_ref(),
                                        run.gate_selection.as_str()
                                    )
                                ),
                            );
                        }
                        run.tasks = parsed.tasks;
                        initialize_task_graph(&mut run);
                        run.synthesis_prompt = parsed.synthesis_prompt;
                        run.stage = SwarmStage::Executing;
                        update_mission_phase(state, &run.mission_id, MissionPhase::Execute);
                        refresh_task_readiness(&mut run);
                        dispatches.extend(dispatch_ready_tasks(&mut run));
                        if let Some(deadlock) = maybe_resolve_deadlock(&mut run) {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                deadlock.message,
                            );
                        }
                        let done = tasks_terminal_count(&run.tasks);
                        update_mission_status(state, &run, Some(done));
                        if done == run.tasks.len() {
                            if let (Some(bundle), Some(verifier)) =
                                (run.gate_bundle.clone(), run.verifier_agent_id.clone())
                            {
                                run.stage = SwarmStage::Verifying;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Verify);
                                update_mission_status(state, &run, Some(done));
                                if state.settings.genome.genome_gate_enabled {
                                    run.genome_gate_results = Some(evaluate_genome_gate(state));
                                }
                                push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    format!(
                                        "Starting VERIFY ({}) on agent {verifier}",
                                        bundle.label()
                                    ),
                                );
                                let prompt = build_verify_prompt(&run, &bundle);
                                dispatches.push(SwarmDispatch {
                                    agent_id: verifier,
                                    mission_id: run.mission_id.clone(),
                                    prompt,
                                    task_role: None,
                                });
                            } else {
                                run.stage = SwarmStage::Synthesizing;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                                update_mission_status(state, &run, Some(done));
                                let prompt = build_synthesis_prompt(&run);
                                dispatches.push(SwarmDispatch {
                                    agent_id: run.planner_agent_id.clone(),
                                    mission_id: run.mission_id.clone(),
                                    prompt,
                                    task_role: None,
                                });
                            }
                        }
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Executing => {
                        let agent_has_writes = state
                            .genome_turn_modified
                            .get(agent_id)
                            .map_or(false, |files| !files.is_empty());
                        if let Some(completed) = mark_task_finished(
                            &mut run,
                            agent_id,
                            message.clone(),
                            false,
                            agent_has_writes,
                        ) {
                            outcome.artifact_focus = Some(SwarmArtifactFocus::Task {
                                mission_id: run.mission_id.clone(),
                                task_id: completed.task_id.clone(),
                            });
                            if completed.expected_artifacts_missing {
                                push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    format!(
                                        "Swarm artifacts: task '{}' declared artifacts but no parseable swarm_artifacts JSON block was found.",
                                        completed.task_id
                                    ),
                                );
                            }
                            if completed.writes_expected && !completed.writes_detected {
                                push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    format!(
                                        "WARNING: task '{}' was expected to write files but no file modifications were detected. \
                                         The agent may have described changes without executing them.",
                                        completed.task_id
                                    ),
                                );
                            }
                        }
                        refresh_task_readiness(&mut run);
                        dispatches.extend(dispatch_ready_tasks(&mut run));
                        if let Some(deadlock) = maybe_resolve_deadlock(&mut run) {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                deadlock.message,
                            );
                        }
                        let done = tasks_terminal_count(&run.tasks);
                        update_mission_status(state, &run, Some(done));
                        if done == run.tasks.len() {
                            if let (Some(bundle), Some(verifier)) =
                                (run.gate_bundle.clone(), run.verifier_agent_id.clone())
                            {
                                run.stage = SwarmStage::Verifying;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Verify);
                                update_mission_status(state, &run, Some(done));
                                if state.settings.genome.genome_gate_enabled {
                                    run.genome_gate_results = Some(evaluate_genome_gate(state));
                                }
                                push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    format!(
                                        "Starting VERIFY ({}) on agent {verifier}",
                                        bundle.label()
                                    ),
                                );
                                let prompt = build_verify_prompt(&run, &bundle);
                                dispatches.push(SwarmDispatch {
                                    agent_id: verifier,
                                    mission_id: run.mission_id.clone(),
                                    prompt,
                                    task_role: None,
                                });
                            } else {
                                run.stage = SwarmStage::Synthesizing;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                                update_mission_status(state, &run, Some(done));
                                let prompt = build_synthesis_prompt(&run);
                                dispatches.push(SwarmDispatch {
                                    agent_id: run.planner_agent_id.clone(),
                                    mission_id: run.mission_id.clone(),
                                    prompt,
                                    task_role: None,
                                });
                            }
                        }
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Verifying => {
                        if run.verifier_agent_id.as_deref() != Some(agent_id.as_str()) {
                            self.runs.insert(mid.clone(), run);
                            return outcome;
                        }
                        run.gate_output = Some(message.clone());
                        run.gate_report = parse_gate_report(message);
                        if let Some(report) = run.gate_report.as_ref() {
                            let outcome = if report.overall_ok { "PASS" } else { "FAIL" };
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                format!("VERIFY result: {outcome}"),
                            );
                            let gate_summary = report
                                .gates
                                .iter()
                                .map(|gate| format!("{} {}", gate.name, gate.ui_status()))
                                .collect::<Vec<_>>()
                                .join(" | ");
                            if !gate_summary.is_empty() {
                                push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    format!("Swarm gates: {gate_summary}"),
                                );
                            }
                        } else {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                "VERIFY result: ERROR (no parseable JSON report)".into(),
                            );
                        }

                        // Dispatch genome reviewer if enabled.
                        if state.settings.genome.genome_gate_enabled {
                            if let Some(reviewer_id) = run.verifier_agent_id.clone() {
                                let review_prompt = build_genome_review_prompt(state);
                                if !review_prompt.is_empty() {
                                    push_system_message_to_mission(
                                        state,
                                        &run.mission_id,
                                        format!("Dispatching genome review to {reviewer_id}"),
                                    );
                                    dispatches.push(SwarmDispatch {
                                        agent_id: reviewer_id,
                                        mission_id: run.mission_id.clone(),
                                        prompt: review_prompt,
                                        task_role: Some("genome-reviewer".into()),
                                    });
                                }
                            }
                        }

                        run.stage = SwarmStage::Synthesizing;
                        update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                        update_mission_status(state, &run, Some(tasks_terminal_count(&run.tasks)));
                        let prompt = build_synthesis_prompt(&run);
                        dispatches.push(SwarmDispatch {
                            agent_id: run.planner_agent_id.clone(),
                            mission_id: run.mission_id.clone(),
                            prompt,
                            task_role: None,
                        });
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Synthesizing if agent_id == &run.planner_agent_id => {
                        run.report_status = Some("DONE".into());
                        run.report_output = Some(message.clone());
                        // Tag the message as a synthesis report.
                        tag_last_agent_message_kind(state, agent_id, &run.mission_id, "synth");
                        outcome.artifact_focus = Some(SwarmArtifactFocus::Report {
                            mission_id: run.mission_id.clone(),
                        });
                        run.stage = SwarmStage::Synthesizing;
                        update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                        let tasks_ok = run
                            .tasks
                            .iter()
                            .all(|task| matches!(task.state, SwarmTaskState::Done));
                        let verify_ok = run.gate_bundle.is_none()
                            || run
                                .gate_report
                                .as_ref()
                                .is_some_and(|report| report.overall_ok);
                        let verify_error = run.gate_bundle.is_some() && run.gate_report.is_none();
                        let final_status = if verify_error {
                            "ERROR"
                        } else if !tasks_ok {
                            "FAILED"
                        } else if verify_ok {
                            "DONE"
                        } else {
                            "FAILED"
                        };
                        update_mission_final(state, &run.mission_id, final_status);
                        cleanup_swarm_clones_for_mission(state, &run.mission_id);
                        self.completed_runs.insert(mid.clone(), run);
                    }
                    _ => {
                        self.runs.insert(mid.clone(), run);
                    }
                }
            }
            AgentBusEvent::TurnFailed {
                agent_id,
                mission_id: Some(mid),
                message,
                ..
            } => {
                let Some(mut run) = self.runs.remove(mid) else {
                    return outcome;
                };

                match run.stage {
                    SwarmStage::Planning if agent_id == &run.planner_agent_id => {
                        tag_last_agent_message_kind(state, agent_id, &run.mission_id, "plan");
                        let available = run
                            .agent_ids
                            .iter()
                            .filter(|id| *id != &run.planner_agent_id)
                            .cloned()
                            .collect::<Vec<_>>();
                        let mut parsed = fallback_tasks(
                            run.template,
                            run.mission_kind,
                            &run.root_prompt,
                            &available,
                            Some(message),
                            run.integrator_agent_id.as_deref(),
                        );
                        parsed.warnings.extend(apply_role_dependency_ordering(
                            state.workspace_root.as_path(),
                            &state.agents.swarm_role_by_agent_id,
                            run.mission_kind,
                            run.integrator_agent_id.as_deref(),
                            parsed.tasks.as_mut_slice(),
                            run.scope_files.len() > 15,
                        ));
                        parsed.warnings.extend(ensure_integrate_task(
                            &mut parsed.tasks,
                            run.mission_kind,
                            run.integrator_agent_id
                                .as_deref()
                                .or(parsed.integrator_agent_id.as_deref()),
                        ));

                        let dag_mode = match read_workspace_dag_validation_mode(
                            state.workspace_root.as_path(),
                        ) {
                            Ok(Some(mode)) => mode,
                            Ok(None) => DEFAULT_DAG_VALIDATION_MODE,
                            Err(err) => {
                                push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    format!(
                                        "PLAN warning: DAG validation config error: {err}; using default mode 'strict'."
                                    ),
                                );
                                DEFAULT_DAG_VALIDATION_MODE
                            }
                        };

                        let mut abort_execution = false;
                        let dag_issues = analyze_swarm_dag(parsed.tasks.as_slice());
                        if !dag_issues.is_empty() {
                            match dag_mode {
                                SwarmDagValidationMode::Strict => {
                                    push_system_message_to_mission(
                                        state,
                                        &run.mission_id,
                                        format!(
                                            "PLAN error: invalid task DAG ({}). Set `[swarm] dag_validation = \"repair\"` in `.nit/config.toml` to auto-repair.",
                                            dag_issues.summary()
                                        ),
                                    );
                                    for task in parsed.tasks.iter_mut() {
                                        if task.state.is_terminal() {
                                            continue;
                                        }
                                        task.state = SwarmTaskState::Skipped;
                                        task.failed = true;
                                        if task.output.is_none() {
                                            task.output = Some(
                                                "SKIPPED (preflight: invalid task DAG)".into(),
                                            );
                                        }
                                    }
                                    abort_execution = true;
                                }
                                SwarmDagValidationMode::Repair => {
                                    let mut warnings =
                                        repair_swarm_dag(parsed.tasks.as_mut_slice());
                                    let after = analyze_swarm_dag(parsed.tasks.as_slice());
                                    if !after.is_empty() {
                                        push_system_message_to_mission(
                                            state,
                                            &run.mission_id,
                                            format!(
                                                "PLAN error: DAG auto-repair failed ({}); aborting.",
                                                after.summary()
                                            ),
                                        );
                                        for task in parsed.tasks.iter_mut() {
                                            if task.state.is_terminal() {
                                                continue;
                                            }
                                            task.state = SwarmTaskState::Skipped;
                                            task.failed = true;
                                            if task.output.is_none() {
                                                task.output = Some(
                                                    "SKIPPED (preflight: DAG auto-repair failed)"
                                                        .into(),
                                                );
                                            }
                                        }
                                        abort_execution = true;
                                    } else if warnings.is_empty() {
                                        warnings.push(
                                            "DAG repair: plan had DAG issues; no changes needed."
                                                .into(),
                                        );
                                    }
                                    parsed.warnings.append(&mut warnings);
                                }
                            }
                        }

                        for warning in parsed.warnings.iter() {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                format!("PLAN warning: {warning}"),
                            );
                        }
                        if abort_execution {
                            abort_swarm_plan_preflight(state, &mut run, parsed);
                            cleanup_swarm_clones_for_mission(state, &run.mission_id);
                            self.completed_runs.insert(mid.clone(), run);
                            return outcome;
                        }
                        let prev_integrator = run.integrator_agent_id.clone();
                        let prev_verifier = run.verifier_agent_id.clone();
                        if parsed.integrator_agent_id.is_some() {
                            run.integrator_agent_id = parsed.integrator_agent_id.clone();
                        }
                        if run.gate_bundle.is_some() {
                            run.verifier_agent_id = {
                                if let Some(integrator) = run.integrator_agent_id.as_deref() {
                                    run.agent_ids
                                        .iter()
                                        .find(|id| {
                                            id.as_str() != run.planner_agent_id.as_str()
                                                && id.as_str() != integrator
                                        })
                                        .cloned()
                                        .or_else(|| {
                                            run.agent_ids
                                                .iter()
                                                .find(|id| {
                                                    id.as_str() != run.planner_agent_id.as_str()
                                                })
                                                .cloned()
                                        })
                                } else {
                                    run.agent_ids
                                        .iter()
                                        .find(|id| id.as_str() != run.planner_agent_id.as_str())
                                        .cloned()
                                }
                            };
                        }
                        if prev_integrator != run.integrator_agent_id
                            || prev_verifier != run.verifier_agent_id
                        {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                format!(
                                    "Swarm template: {} | integrator: {} | verifier: {} | gates: {}",
                                    run.template.label(),
                                    run.integrator_agent_id
                                        .as_deref()
                                        .unwrap_or("(none)"),
                                    run.verifier_agent_id.as_deref().unwrap_or("(none)"),
                                    gate_bundle_label(
                                        run.gate_bundle.as_ref(),
                                        run.gate_selection.as_str()
                                    )
                                ),
                            );
                        }
                        run.tasks = parsed.tasks;
                        run.synthesis_prompt = parsed.synthesis_prompt;
                        run.stage = SwarmStage::Executing;
                        update_mission_phase(state, &run.mission_id, MissionPhase::Execute);
                        initialize_task_graph(&mut run);
                        refresh_task_readiness(&mut run);
                        dispatches.extend(dispatch_ready_tasks(&mut run));
                        if let Some(deadlock) = maybe_resolve_deadlock(&mut run) {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                deadlock.message,
                            );
                        }
                        let done = tasks_terminal_count(&run.tasks);
                        update_mission_status(state, &run, Some(done));
                        if done == run.tasks.len() {
                            if let (Some(bundle), Some(verifier)) =
                                (run.gate_bundle.clone(), run.verifier_agent_id.clone())
                            {
                                run.stage = SwarmStage::Verifying;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Verify);
                                update_mission_status(state, &run, Some(done));
                                if state.settings.genome.genome_gate_enabled {
                                    run.genome_gate_results = Some(evaluate_genome_gate(state));
                                }
                                push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    format!(
                                        "Starting VERIFY ({}) on agent {verifier}",
                                        bundle.label()
                                    ),
                                );
                                let prompt = build_verify_prompt(&run, &bundle);
                                dispatches.push(SwarmDispatch {
                                    agent_id: verifier,
                                    mission_id: run.mission_id.clone(),
                                    prompt,
                                    task_role: None,
                                });
                            } else {
                                run.stage = SwarmStage::Synthesizing;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                                update_mission_status(state, &run, Some(done));
                                let prompt = build_synthesis_prompt(&run);
                                dispatches.push(SwarmDispatch {
                                    agent_id: run.planner_agent_id.clone(),
                                    mission_id: run.mission_id.clone(),
                                    prompt,
                                    task_role: None,
                                });
                            }
                        }
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Executing => {
                        let agent_has_writes = state
                            .genome_turn_modified
                            .get(agent_id)
                            .map_or(false, |files| !files.is_empty());

                        // Retry write-role tasks once on failure — the agent may
                        // have crashed due to a transient issue (rate limit,
                        // context overflow, etc.).  Reset the task to Ready so it
                        // gets re-dispatched with its dependency outputs.
                        let mut retried = false;
                        let task_idx = run.tasks.iter().position(|t| {
                            t.agent_id == *agent_id
                                && matches!(
                                    t.state,
                                    SwarmTaskState::Running | SwarmTaskState::Dispatched
                                )
                        });
                        if let Some(idx) = task_idx {
                            let task = &mut run.tasks[idx];
                            if task.writes && task.retries < 1 {
                                task.retries += 1;
                                task.state = SwarmTaskState::Ready;
                                task.output = None;
                                push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    format!(
                                        "Task '{}' failed ({}); retrying (attempt {}).",
                                        task.id,
                                        message.chars().take(120).collect::<String>(),
                                        task.retries + 1,
                                    ),
                                );
                                retried = true;
                            }
                        }

                        if retried {
                            refresh_task_readiness(&mut run);
                            dispatches.extend(dispatch_ready_tasks(&mut run));
                            self.runs.insert(mid.clone(), run);
                        } else {
                            if let Some(completed) = mark_task_finished(
                                &mut run,
                                agent_id,
                                message.clone(),
                                true,
                                agent_has_writes,
                            ) {
                                outcome.artifact_focus = Some(SwarmArtifactFocus::Task {
                                    mission_id: run.mission_id.clone(),
                                    task_id: completed.task_id.clone(),
                                });
                                if completed.expected_artifacts_missing {
                                    push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    format!(
                                        "Swarm artifacts: task '{}' declared artifacts but no parseable swarm_artifacts JSON block was found.",
                                        completed.task_id
                                    ),
                                );
                                }
                                if completed.writes_expected && !completed.writes_detected {
                                    push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    format!(
                                        "WARNING: task '{}' was expected to write files but no file modifications were detected. \
                                         The agent may have described changes without executing them.",
                                        completed.task_id
                                    ),
                                );
                                }
                            }
                            // Drain orphaned queued turns for the failed agent so
                            // queue_len doesn't leak.
                            drain_queued_turns_for_agent(state, agent_id);
                            refresh_task_readiness(&mut run);
                            dispatches.extend(dispatch_ready_tasks(&mut run));
                            if let Some(deadlock) = maybe_resolve_deadlock(&mut run) {
                                push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    deadlock.message,
                                );
                            }
                            let done = tasks_terminal_count(&run.tasks);
                            update_mission_status(state, &run, Some(done));
                            if done == run.tasks.len() {
                                if let (Some(bundle), Some(verifier)) =
                                    (run.gate_bundle.clone(), run.verifier_agent_id.clone())
                                {
                                    run.stage = SwarmStage::Verifying;
                                    update_mission_phase(
                                        state,
                                        &run.mission_id,
                                        MissionPhase::Verify,
                                    );
                                    update_mission_status(state, &run, Some(done));
                                    if state.settings.genome.genome_gate_enabled {
                                        run.genome_gate_results = Some(evaluate_genome_gate(state));
                                    }
                                    push_system_message_to_mission(
                                        state,
                                        &run.mission_id,
                                        format!(
                                            "Starting VERIFY ({}) on agent {verifier}",
                                            bundle.label()
                                        ),
                                    );
                                    let prompt = build_verify_prompt(&run, &bundle);
                                    dispatches.push(SwarmDispatch {
                                        agent_id: verifier,
                                        mission_id: run.mission_id.clone(),
                                        prompt,
                                        task_role: None,
                                    });
                                } else {
                                    run.stage = SwarmStage::Synthesizing;
                                    update_mission_phase(
                                        state,
                                        &run.mission_id,
                                        MissionPhase::Report,
                                    );
                                    update_mission_status(state, &run, Some(done));
                                    let prompt = build_synthesis_prompt(&run);
                                    dispatches.push(SwarmDispatch {
                                        agent_id: run.planner_agent_id.clone(),
                                        mission_id: run.mission_id.clone(),
                                        prompt,
                                        task_role: None,
                                    });
                                }
                            }
                            self.runs.insert(mid.clone(), run);
                        }
                    }
                    SwarmStage::Verifying => {
                        if run.verifier_agent_id.as_deref() != Some(agent_id.as_str()) {
                            self.runs.insert(mid.clone(), run);
                            return outcome;
                        }
                        run.gate_output = Some(message.clone());
                        run.gate_report = None;
                        push_system_message_to_mission(
                            state,
                            &run.mission_id,
                            format!("VERIFY result: ERROR ({message})"),
                        );

                        // Dispatch genome reviewer if enabled.
                        if state.settings.genome.genome_gate_enabled {
                            if let Some(reviewer_id) = run.verifier_agent_id.clone() {
                                let review_prompt = build_genome_review_prompt(state);
                                if !review_prompt.is_empty() {
                                    push_system_message_to_mission(
                                        state,
                                        &run.mission_id,
                                        format!("Dispatching genome review to {reviewer_id}"),
                                    );
                                    dispatches.push(SwarmDispatch {
                                        agent_id: reviewer_id,
                                        mission_id: run.mission_id.clone(),
                                        prompt: review_prompt,
                                        task_role: Some("genome-reviewer".into()),
                                    });
                                }
                            }
                        }

                        run.stage = SwarmStage::Synthesizing;
                        update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                        update_mission_status(state, &run, Some(tasks_terminal_count(&run.tasks)));
                        let prompt = build_synthesis_prompt(&run);
                        dispatches.push(SwarmDispatch {
                            agent_id: run.planner_agent_id.clone(),
                            mission_id: run.mission_id.clone(),
                            prompt,
                            task_role: None,
                        });
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Synthesizing if agent_id == &run.planner_agent_id => {
                        run.report_status = Some("ERROR".into());
                        run.report_output = Some(message.clone());
                        tag_last_agent_message_kind(state, agent_id, &run.mission_id, "synth");
                        outcome.artifact_focus = Some(SwarmArtifactFocus::Report {
                            mission_id: run.mission_id.clone(),
                        });
                        update_mission_final(state, &run.mission_id, "ERROR");
                        cleanup_swarm_clones_for_mission(state, &run.mission_id);
                        self.completed_runs.insert(mid.clone(), run);
                    }
                    _ => {
                        self.runs.insert(mid.clone(), run);
                    }
                }
            }
            _ => {}
        }

        outcome
    }

    fn run_for_mission(&self, mission_id: &str) -> Option<&SwarmRun> {
        self.runs
            .get(mission_id)
            .or_else(|| self.completed_runs.get(mission_id))
    }
}

#[derive(serde::Deserialize)]
struct SwarmPlanV2 {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    template: Option<String>,
    #[serde(default)]
    integrator_agent_id: Option<String>,
    tasks: Vec<SwarmPlanTaskV2>,
    #[serde(default)]
    synthesis_prompt: Option<String>,
}

#[derive(serde::Deserialize)]
struct SwarmPlanTaskV2 {
    #[serde(default)]
    id: Option<String>,
    agent_id: String,
    #[serde(default)]
    role: Option<String>,
    title: String,
    prompt: String,
    #[serde(default)]
    deps: Vec<String>,
    #[serde(default)]
    writes: bool,
    #[serde(default)]
    artifacts: Vec<String>,
    #[serde(default)]
    done_when: Option<String>,
}

#[derive(serde::Deserialize)]
struct SwarmPlanV1 {
    tasks: Vec<SwarmPlanTaskV1>,
    #[serde(default)]
    synthesis_prompt: Option<String>,
}

#[derive(serde::Deserialize)]
struct SwarmPlanTaskV1 {
    agent_id: String,
    title: String,
    prompt: String,
}

struct ParsedSwarmPlan {
    tasks: Vec<SwarmTask>,
    synthesis_prompt: Option<String>,
    integrator_agent_id: Option<String>,
    warnings: Vec<String>,
}

fn task_role_is(task: &SwarmTask, role: &str) -> bool {
    task.role
        .as_deref()
        .is_some_and(|r| r.trim().eq_ignore_ascii_case(role))
}

fn bulk_is_proposer(task: &SwarmTask) -> bool {
    task_role_is(task, "propose") || task.id.to_ascii_lowercase().starts_with("propose-")
}

fn bulk_is_judge(task: &SwarmTask) -> bool {
    task_role_is(task, "judge") || task.id.eq_ignore_ascii_case("judge")
}

fn bulk_is_integrate(task: &SwarmTask) -> bool {
    task_role_is(task, "integrate") || task.id.eq_ignore_ascii_case("integrate")
}

fn normalize_bulk_plan(tasks: &mut [SwarmTask], integrator_agent_id: Option<&str>) -> Vec<String> {
    let mut warnings = Vec::new();
    let proposer_ids = tasks
        .iter()
        .filter(|task| bulk_is_proposer(task))
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    let judge_idx = tasks.iter().position(bulk_is_judge);
    let integrate_idx = tasks.iter().position(bulk_is_integrate);

    for task in tasks.iter_mut() {
        if bulk_is_proposer(task) && task.writes {
            task.writes = false;
            warnings.push(format!(
                "Bulk plan: proposer task '{}' had writes=true; forcing read-only.",
                task.id
            ));
        }
    }

    if let Some(judge_idx) = judge_idx {
        let judge_id = tasks[judge_idx].id.clone();
        let mut changed = false;
        for proposer in proposer_ids.iter() {
            if proposer == &judge_id {
                continue;
            }
            if tasks[judge_idx].deps.iter().any(|dep| dep == proposer) {
                continue;
            }
            tasks[judge_idx].deps.push(proposer.clone());
            changed = true;
        }
        if changed {
            warnings.push(
                "Bulk plan: added missing deps so the judge depends on all proposer tasks.".into(),
            );
        }
    }

    if let (Some(integrate_idx), Some(judge_idx)) = (integrate_idx, judge_idx) {
        let judge_id = tasks[judge_idx].id.clone();
        if !tasks[integrate_idx].deps.iter().any(|dep| dep == &judge_id) {
            tasks[integrate_idx].deps.push(judge_id);
            warnings.push("Bulk plan: added missing dep so integrate depends on judge.".into());
        }
    }

    if let Some(integrate_idx) = integrate_idx {
        let allowed = integrator_agent_id
            .is_none_or(|integrator| tasks[integrate_idx].agent_id == integrator);
        if allowed && !tasks[integrate_idx].writes {
            tasks[integrate_idx].writes = true;
            warnings
                .push("Bulk plan: forcing integrate task writes=true for the integrator.".into());
        }
    }

    warnings
}

/// Safety net: if the planner omitted an integrate task for a General mission,
/// inject one so the swarm can actually write to the workspace.
fn ensure_integrate_task(
    tasks: &mut Vec<SwarmTask>,
    mission_kind: SwarmMissionKind,
    integrator_agent_id: Option<&str>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if !matches!(mission_kind, SwarmMissionKind::General) {
        return warnings;
    }
    let Some(integrator) = integrator_agent_id else {
        return warnings;
    };

    let has_integrate = tasks
        .iter()
        .any(|t| t.role.as_deref().and_then(normalize_role_label).as_deref() == Some("integrate"));
    if has_integrate {
        return warnings;
    }

    // Check if any task on the integrator agent can be promoted.
    let promote_idx = tasks
        .iter()
        .position(|t| t.agent_id == integrator && t.role.is_none());
    if let Some(idx) = promote_idx {
        tasks[idx].role = Some("integrate".into());
        tasks[idx].writes = true;
        warnings.push(format!(
            "Plan safety net: promoted task '{}' to role=integrate (writes=true) because no integrate task was found.",
            tasks[idx].id
        ));
        return warnings;
    }

    // No promotable task — inject a new integrate task that depends on all others.
    let all_deps: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
    tasks.push(SwarmTask {
        id: "integrate".into(),
        agent_id: integrator.to_string(),
        role: Some("integrate".into()),
        title: "Integrate + implement".into(),
        task_prompt: "Implement the changes using the dependency outputs. You are the only agent allowed to make workspace edits. Process the FILE CHECKLIST above in order — open each file, refactor it, then move to the next. Prefer small, safe diffs and keep tests green.".into(),
        deps: all_deps,
        writes: true,
        artifacts: Vec::new(),
        done_when: Some("Changes are implemented cleanly with validations to run.".into()),
        state: SwarmTaskState::Pending,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
    });
    warnings
        .push("Plan safety net: injected integrate task because the planner omitted one.".into());
    warnings
}

fn validate_bulk_plan(
    tasks: &[SwarmTask],
    available_agents: &[String],
    integrator_agent_id: Option<&str>,
) -> Result<(), String> {
    let mut issues = Vec::new();
    let proposer_tasks = tasks
        .iter()
        .filter(|task| bulk_is_proposer(task))
        .collect::<Vec<_>>();
    let judge_task = tasks.iter().find(|task| bulk_is_judge(task));
    let integrate_task = tasks.iter().find(|task| bulk_is_integrate(task));

    if proposer_tasks.is_empty() {
        issues.push("missing proposer tasks (role=propose or id=propose-XX)".into());
    }
    if judge_task.is_none() {
        issues.push("missing judge task (role=judge or id=judge)".into());
    }
    if integrate_task.is_none() {
        issues.push("missing integrate task (role=integrate or id=integrate)".into());
    }

    if let Some(integrate_task) = integrate_task {
        if !integrate_task.writes {
            issues.push("integrate task must set writes=true (integrator step)".into());
        }
        if let Some(integrator) = integrator_agent_id {
            if integrate_task.agent_id != integrator {
                issues.push(format!(
                    "integrate task must be assigned to integrator agent '{integrator}' (got '{}')",
                    integrate_task.agent_id
                ));
            }
        }
    }

    if let Some(judge_task) = judge_task {
        for proposer in proposer_tasks.iter() {
            if proposer.id == judge_task.id {
                continue;
            }
            if !judge_task.deps.iter().any(|dep| dep == &proposer.id) {
                issues.push(format!(
                    "judge task must depend on proposer task '{}' (missing dep)",
                    proposer.id
                ));
            }
        }
    }

    if let (Some(judge_task), Some(integrate_task)) = (judge_task, integrate_task) {
        if !integrate_task.deps.iter().any(|dep| dep == &judge_task.id) {
            issues.push("integrate task must depend on judge task".into());
        }
    }

    let non_integrator_agents = match integrator_agent_id {
        Some(integrator) => available_agents
            .iter()
            .filter(|id| id.as_str() != integrator)
            .count(),
        None => available_agents.len(),
    };
    let min_proposers = if non_integrator_agents >= 2 { 2 } else { 1 };
    if proposer_tasks.len() < min_proposers {
        issues.push(format!(
            "expected at least {min_proposers} proposer tasks for bulk orchestration"
        ));
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues.join("; "))
    }
}

#[derive(Copy, Clone, Debug, Default)]
struct RoleDepStats {
    added: usize,
    skipped_cycle: usize,
}

pub(crate) fn normalize_role_label(raw: &str) -> Option<String> {
    let role = raw.trim().to_ascii_lowercase();
    if role.is_empty() {
        return None;
    }
    if role.eq_ignore_ascii_case("all") {
        return None;
    }
    if role.eq_ignore_ascii_case(COMPUTATIONAL_RESEARCH_ROLE_LEGACY) {
        return Some(COMPUTATIONAL_RESEARCH_ROLE.into());
    }
    Some(role)
}

fn role_is_singleton(role: &str) -> bool {
    matches!(
        normalize_role_label(role).as_deref(),
        Some("judge" | "integrate")
    )
}

fn role_requires_research_intent(role: &str) -> bool {
    matches!(
        normalize_role_label(role).as_deref(),
        Some("research" | COMPUTATIONAL_RESEARCH_ROLE)
    )
}

fn prompt_contains_any(prompt: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| prompt.contains(needle))
}

fn prompt_explicitly_requests_research_role(prompt: &str) -> bool {
    prompt_contains_any(
        prompt,
        &[
            "mission=research",
            "mission: research",
            "use research",
            "assign research",
            "need research",
            "want research",
            "with research role",
            "research agent",
            "research lane",
        ],
    )
}

fn prompt_explicitly_requests_computational_research_role(prompt: &str) -> bool {
    prompt_contains_any(
        prompt,
        &[
            "mission=computational",
            "mission=computational-research",
            "mission: computational",
            "mission: computational-research",
            "mission: computational research",
            "use computational research",
            "use computational-research",
            "assign computational research",
            "assign computational-research",
            "need computational research",
            "need computational-research",
            "want computational research",
            "want computational-research",
            "with computational research role",
            "with computational-research role",
            "computational research agent",
            "computational-research agent",
            "computational research lane",
            "computational-research lane",
        ],
    )
}

fn prompt_has_research_intent(prompt: &str) -> bool {
    if prompt_contains_any(
        prompt,
        &[
            "do research",
            "conduct research",
            "research the",
            "research this topic",
            "survey the literature",
            "literature review",
            "read papers",
            "read the papers",
            "search the web",
            "browse the web",
            "search online",
            "find sources",
            "find references",
            "gather citations",
            "prior art",
            "related work",
            "explore ideas",
            "explore topics",
            "new ideas",
        ],
    ) {
        return true;
    }

    prompt_contains_any(
        prompt,
        &[
            "research",
            "investigate",
            "survey",
            "study",
            "search",
            "browse",
            "read",
            "compare",
            "evaluate",
            "explore",
        ],
    ) && prompt_contains_any(
        prompt,
        &[
            "papers",
            "literature",
            "web",
            "online",
            "sources",
            "references",
            "citations",
            "resources",
            "prior art",
            "related work",
            "topic",
            "topics",
            "ideas",
            "hypothesis",
            "hypotheses",
        ],
    )
}

fn prompt_has_computational_research_intent(prompt: &str) -> bool {
    if prompt_contains_any(
        prompt,
        &[
            "computational research",
            "run simulations",
            "build a model",
            "model this",
            "numerical study",
            "optimization study",
            "design an experiment",
            "reproducible analysis",
        ],
    ) {
        return true;
    }

    prompt_contains_any(
        prompt,
        &[
            "simulation",
            "simulate",
            "modeling",
            "modelling",
            "numerical",
            "optimization",
            "optimisation",
            "data fitting",
            "model fitting",
            "network analysis",
            "pattern analysis",
            "reproducible",
            "benchmark",
            "experiment",
            "measurement",
        ],
    ) && prompt_contains_any(
        prompt,
        &[
            "research",
            "study",
            "evaluate",
            "compare",
            "topic",
            "topics",
            "hypothesis",
            "hypotheses",
            "papers",
            "literature",
            "sources",
            "evidence",
            "dataset",
            "datasets",
            "methods",
        ],
    )
}

pub(crate) fn detect_swarm_mission_kind_from_prompt(root_prompt: &str) -> Option<SwarmMissionKind> {
    let prompt = root_prompt.trim().to_ascii_lowercase();
    if prompt.is_empty() {
        return None;
    }

    if let Some(kind) = explicit_swarm_mission_kind_from_prompt(root_prompt) {
        return Some(kind);
    }

    if prompt_explicitly_requests_computational_research_role(prompt.as_str())
        || prompt_has_computational_research_intent(prompt.as_str())
    {
        return Some(SwarmMissionKind::ComputationalResearch);
    }

    if prompt_explicitly_requests_research_role(prompt.as_str())
        || prompt_has_research_intent(prompt.as_str())
    {
        return Some(SwarmMissionKind::Research);
    }

    None
}

fn classify_swarm_mission_kind(
    root_prompt: &str,
    explicit: Option<SwarmMissionKind>,
) -> SwarmMissionKind {
    explicit
        .or_else(|| detect_swarm_mission_kind_from_prompt(root_prompt))
        .unwrap_or(SwarmMissionKind::General)
}

fn role_allowed_for_mission(mission_kind: SwarmMissionKind, role: &str) -> bool {
    if !role_requires_research_intent(role) {
        return true;
    }
    mission_kind.allows_role(role)
}

fn direct_role_hint_for_agent(
    role_hints_by_agent_id: &HashMap<String, String>,
    agent_id: &str,
) -> Option<String> {
    role_hints_by_agent_id
        .get(agent_id)
        .and_then(|hint| normalize_role_label(hint.as_str()))
}

fn inherited_clone_role_hint_for_agent(
    role_hints_by_agent_id: &HashMap<String, String>,
    agent_id: &str,
) -> Option<String> {
    let base_id = swarm_clone_base_id(agent_id)?;
    let hint = direct_role_hint_for_agent(role_hints_by_agent_id, base_id)?;
    (!role_is_singleton(hint.as_str())).then_some(hint)
}

fn inferred_role_hint_for_agent(
    role_hints_by_agent_id: &HashMap<String, String>,
    agent_id: &str,
    integrator_agent_id: Option<&str>,
    mission_kind: SwarmMissionKind,
) -> Option<String> {
    let hint = direct_role_hint_for_agent(role_hints_by_agent_id, agent_id)
        .or_else(|| inherited_clone_role_hint_for_agent(role_hints_by_agent_id, agent_id))?;
    if hint == "integrate" && integrator_agent_id.is_some_and(|integrator| integrator != agent_id) {
        return None;
    }
    if !role_allowed_for_mission(mission_kind, hint.as_str()) {
        return None;
    }
    Some(hint)
}

fn planner_role_hint_for_agent(
    role_hints_by_agent_id: &HashMap<String, String>,
    agent_id: &str,
    integrator_agent_id: Option<&str>,
    mission_kind: SwarmMissionKind,
) -> String {
    inferred_role_hint_for_agent(
        role_hints_by_agent_id,
        agent_id,
        integrator_agent_id,
        mission_kind,
    )
    .unwrap_or_else(|| "all".into())
}

/// Deduplicate inherited role hints so that clones of the same base agent don't
/// all receive the same hint. Only the first clone keeps the inherited hint; the
/// rest get "all" so the planner is free to diversify roles.
fn deduplicate_inherited_role_hints(
    role_hints: &mut [(String, String)],
    role_hints_by_agent_id: &HashMap<String, String>,
) {
    let mut seen_inherited: HashMap<&str, usize> = HashMap::new();
    for (idx, (agent_id, hint)) in role_hints.iter().enumerate() {
        if hint == "all" {
            continue;
        }
        // Check if this hint was inherited (agent has no direct hint but its base does).
        let has_direct = direct_role_hint_for_agent(role_hints_by_agent_id, agent_id).is_some();
        if has_direct {
            continue;
        }
        let Some(base_id) = swarm_clone_base_id(agent_id).or_else(|| chat_clone_base_id(agent_id))
        else {
            continue;
        };
        seen_inherited.entry(base_id).or_insert(idx);
    }
    // Second pass: reset duplicates to "all".
    let mut count_by_base: HashMap<&str, usize> = HashMap::new();
    for (agent_id, hint) in role_hints.iter_mut() {
        if hint == "all" {
            continue;
        }
        let has_direct = direct_role_hint_for_agent(role_hints_by_agent_id, agent_id).is_some();
        if has_direct {
            continue;
        }
        let Some(base_id) = swarm_clone_base_id(agent_id).or_else(|| chat_clone_base_id(agent_id))
        else {
            continue;
        };
        let count = count_by_base.entry(base_id).or_insert(0);
        if *count > 0 {
            *hint = "all".into();
        }
        *count += 1;
    }
}

fn infer_role_from_task_id(task_id: &str) -> Option<&'static str> {
    let id = task_id.trim();
    if id.is_empty() {
        return None;
    }
    if id.to_ascii_lowercase().starts_with("propose-") {
        return Some("propose");
    }
    if id.eq_ignore_ascii_case("judge") {
        return Some("judge");
    }
    if id.eq_ignore_ascii_case("integrate") || id.eq_ignore_ascii_case("implement") {
        return Some("integrate");
    }
    if id.eq_ignore_ascii_case("review") {
        return Some("review");
    }
    if id.eq_ignore_ascii_case("test") {
        return Some("test");
    }
    None
}

fn infer_integrator_agent_id_from_v2_tasks(
    tasks: &[SwarmPlanTaskV2],
    available_agents: &[String],
) -> Option<(String, &'static str)> {
    let normalize_agent_id = |raw: &str| {
        let raw = raw.trim();
        available_agents
            .iter()
            .find(|candidate| candidate.as_str() == raw)
            .cloned()
    };

    let mut integrate_agents = Vec::new();
    let mut writer_agents = Vec::new();
    for task in tasks.iter() {
        let Some(agent_id) = normalize_agent_id(task.agent_id.as_str()) else {
            continue;
        };

        let has_integrate_role = task
            .role
            .as_deref()
            .and_then(normalize_role_label)
            .as_deref()
            == Some("integrate")
            || task
                .id
                .as_deref()
                .and_then(infer_role_from_task_id)
                .is_some_and(|role| role == "integrate");
        if has_integrate_role
            && !integrate_agents
                .iter()
                .any(|existing| existing == &agent_id)
        {
            integrate_agents.push(agent_id.clone());
        }
        if task.writes && !writer_agents.iter().any(|existing| existing == &agent_id) {
            writer_agents.push(agent_id);
        }
    }

    if integrate_agents.len() == 1
        && (writer_agents.is_empty() || writer_agents.iter().all(|id| id == &integrate_agents[0]))
    {
        let reason = if writer_agents.is_empty() {
            "integrate task"
        } else {
            "integrate task + writes=true task"
        };
        return Some((integrate_agents.remove(0), reason));
    }

    if writer_agents.len() == 1 && integrate_agents.is_empty() {
        return Some((writer_agents.remove(0), "writes=true task"));
    }

    None
}

fn default_role_deps() -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    map.insert("consumer".into(), vec!["producer".into()]);
    map.insert(
        "judge".into(),
        vec![
            "research".into(),
            COMPUTATIONAL_RESEARCH_ROLE.into(),
            "propose".into(),
        ],
    );
    map.insert(
        "integrate".into(),
        vec![
            "judge".into(),
            "research".into(),
            COMPUTATIONAL_RESEARCH_ROLE.into(),
            "propose".into(),
        ],
    );
    map.insert("review".into(), vec!["integrate".into()]);
    map.insert("test".into(), vec!["integrate".into()]);
    map
}

fn read_workspace_role_deps(
    workspace_root: &Path,
) -> Result<Option<HashMap<String, Vec<String>>>, String> {
    let path = workspace_root.join(".nit").join("config.toml");
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path)
        .map_err(|err| format!("failed reading {}: {err}", path.display()))?;
    let value = toml::from_str::<toml::Value>(&contents)
        .map_err(|err| format!("failed parsing {}: {err}", path.display()))?;
    let table = value
        .get("swarm")
        .and_then(|value| value.get("role_deps"))
        .and_then(|value| value.as_table());
    let Some(table) = table else {
        return Ok(None);
    };

    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for (consumer, producers) in table.iter() {
        let Some(consumer) = normalize_role_label(consumer) else {
            continue;
        };
        let mut normalized = Vec::new();
        if let Some(producers) = producers.as_array() {
            for producer in producers.iter() {
                let Some(producer) = producer.as_str().and_then(normalize_role_label) else {
                    continue;
                };
                if normalized
                    .iter()
                    .any(|existing: &String| existing == &producer)
                {
                    continue;
                }
                normalized.push(producer);
            }
        } else if let Some(producer) = producers.as_str().and_then(normalize_role_label) {
            normalized.push(producer);
        } else {
            continue;
        }
        if !normalized.is_empty() {
            out.insert(consumer, normalized);
        }
    }

    if out.is_empty() {
        Ok(None)
    } else {
        Ok(Some(out))
    }
}

fn would_create_cycle(
    tasks: &[SwarmTask],
    idx_by_id: &HashMap<String, usize>,
    task_id: &str,
    dep_id: &str,
) -> bool {
    if task_id == dep_id {
        return true;
    }
    let Some(&start) = idx_by_id.get(dep_id) else {
        return false;
    };
    let Some(&target) = idx_by_id.get(task_id) else {
        return false;
    };

    let mut seen: HashSet<usize> = HashSet::new();
    let mut stack = vec![start];
    while let Some(idx) = stack.pop() {
        if idx == target {
            return true;
        }
        if !seen.insert(idx) {
            continue;
        }
        for dep in tasks[idx].deps.iter() {
            if let Some(&next) = idx_by_id.get(dep) {
                stack.push(next);
            }
        }
    }
    false
}

fn apply_role_deps(
    tasks: &mut [SwarmTask],
    role_deps: &HashMap<String, Vec<String>>,
) -> RoleDepStats {
    let mut stats = RoleDepStats::default();
    if tasks.is_empty() || role_deps.is_empty() {
        return stats;
    }

    let idx_by_id = tasks
        .iter()
        .enumerate()
        .map(|(idx, task)| (task.id.clone(), idx))
        .collect::<HashMap<_, _>>();

    let mut tasks_by_role: HashMap<String, Vec<String>> = HashMap::new();
    for task in tasks.iter() {
        if let Some(role) = task.role.as_deref().and_then(normalize_role_label) {
            tasks_by_role.entry(role).or_default().push(task.id.clone());
        }
        if task.writes {
            // Treat writer tasks as integrate-like for role-based ordering.
            let entry = tasks_by_role.entry("integrate".into()).or_default();
            if !entry.iter().any(|id| id == &task.id) {
                entry.push(task.id.clone());
            }
        }
    }

    let mut consumer_roles = role_deps.keys().cloned().collect::<Vec<_>>();
    consumer_roles.sort();
    for consumer_role in consumer_roles.iter() {
        let Some(producer_roles) = role_deps.get(consumer_role) else {
            continue;
        };
        let Some(consumer_task_ids) = tasks_by_role.get(consumer_role) else {
            continue;
        };
        if consumer_task_ids.is_empty() {
            continue;
        }
        for consumer_task_id in consumer_task_ids.iter() {
            let Some(&consumer_idx) = idx_by_id.get(consumer_task_id) else {
                continue;
            };
            for producer_role in producer_roles.iter() {
                let Some(producer_task_ids) = tasks_by_role.get(producer_role) else {
                    continue;
                };
                for producer_task_id in producer_task_ids.iter() {
                    if producer_task_id == consumer_task_id {
                        continue;
                    }
                    if tasks[consumer_idx]
                        .deps
                        .iter()
                        .any(|existing| existing == producer_task_id)
                    {
                        continue;
                    }
                    if would_create_cycle(
                        tasks,
                        &idx_by_id,
                        consumer_task_id.as_str(),
                        producer_task_id.as_str(),
                    ) {
                        stats.skipped_cycle = stats.skipped_cycle.saturating_add(1);
                        continue;
                    }
                    tasks[consumer_idx].deps.push(producer_task_id.clone());
                    stats.added = stats.added.saturating_add(1);
                }
            }
        }
    }

    stats
}

fn apply_role_dependency_ordering(
    workspace_root: &Path,
    role_hints_by_agent_id: &HashMap<String, String>,
    mission_kind: SwarmMissionKind,
    integrator_agent_id: Option<&str>,
    tasks: &mut [SwarmTask],
    multi_integrator: bool,
) -> Vec<String> {
    if tasks.is_empty() {
        return Vec::new();
    }

    let mut warnings = Vec::new();
    let integrator_agent_id = integrator_agent_id
        .map(str::trim)
        .filter(|id| !id.is_empty());

    for task in tasks.iter_mut() {
        let Some(role) = task.role.as_deref().and_then(normalize_role_label) else {
            task.role = None;
            continue;
        };
        if !role_allowed_for_mission(mission_kind, role.as_str()) {
            warnings.push(format!(
                "Role ordering: cleared role '{}' on task '{}' because mission focus '{}' does not permit that research role.",
                role,
                task.id,
                mission_kind.label()
            ));
            task.role = None;
            continue;
        }
        if role == "integrate"
            && !multi_integrator
            && integrator_agent_id.is_some_and(|integrator| task.agent_id != integrator)
        {
            warnings.push(format!(
                "Role ordering: cleared invalid integrate role on task '{}' because agent '{}' is not the integrator.",
                task.id, task.agent_id
            ));
            task.role = None;
            continue;
        }
        task.role = Some(role);
    }

    let mut inferred_roles = 0usize;
    for task in tasks.iter_mut() {
        if task.role.is_some() {
            continue;
        }
        if task.writes {
            task.role = Some("integrate".into());
            inferred_roles = inferred_roles.saturating_add(1);
            continue;
        }
        if let Some(inferred) = infer_role_from_task_id(task.id.as_str()) {
            if inferred == "integrate"
                && !multi_integrator
                && integrator_agent_id.is_some_and(|integrator| task.agent_id != integrator)
            {
                warnings.push(format!(
                    "Role ordering: left task '{}' without role because its id implies integrate but agent '{}' is not the integrator.",
                    task.id, task.agent_id
                ));
                continue;
            }
            task.role = Some(inferred.to_string());
            inferred_roles = inferred_roles.saturating_add(1);
            continue;
        }
        let Some(hint) = inferred_role_hint_for_agent(
            role_hints_by_agent_id,
            task.agent_id.as_str(),
            integrator_agent_id,
            mission_kind,
        ) else {
            continue;
        };
        task.role = Some(hint);
        inferred_roles = inferred_roles.saturating_add(1);
    }

    let (role_deps, source) = match read_workspace_role_deps(workspace_root) {
        Ok(Some(map)) => (map, "config"),
        Ok(None) => (default_role_deps(), "built-in"),
        Err(err) => {
            warnings.push(format!("Role ordering: {err}; using built-in role deps."));
            (default_role_deps(), "built-in")
        }
    };

    let stats = apply_role_deps(tasks, &role_deps);
    if stats.added > 0 || stats.skipped_cycle > 0 {
        let mut parts = Vec::new();
        if inferred_roles > 0 {
            parts.push(format!("inferred {inferred_roles} role(s)"));
        }
        if stats.added > 0 {
            parts.push(format!("added {} dep(s)", stats.added));
        }
        if stats.skipped_cycle > 0 {
            parts.push(format!("skipped {} dep(s) (cycle)", stats.skipped_cycle));
        }
        if parts.is_empty() {
            parts.push("no changes".into());
        }
        warnings.push(format!("Role ordering ({source}): {}.", parts.join(", ")));
    }

    warnings
}

#[allow(clippy::too_many_arguments)]
fn parse_plan_from_planner(
    planner_message: &str,
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    root_prompt: &str,
    available_agents: &[String],
    integrator_hint: Option<&str>,
    integrator_locked: bool,
    multi_integrator: bool,
) -> ParsedSwarmPlan {
    let Some(json) = extract_json_code_block(planner_message) else {
        return fallback_tasks(
            template,
            mission_kind,
            root_prompt,
            available_agents,
            None,
            integrator_hint,
        );
    };

    if let Ok(plan) = serde_json::from_str::<SwarmPlanV2>(&json) {
        if let Some(mut parsed) = parse_v2_plan(
            plan,
            template,
            available_agents,
            integrator_hint,
            integrator_locked,
            multi_integrator,
        ) {
            if matches!(template, SwarmTemplate::Bulk) {
                let integrator = parsed
                    .integrator_agent_id
                    .as_deref()
                    .or(integrator_hint)
                    .or_else(|| available_agents.first().map(String::as_str));
                let mut warnings = normalize_bulk_plan(&mut parsed.tasks, integrator);
                parsed.warnings.append(&mut warnings);
                if let Err(issue) = validate_bulk_plan(&parsed.tasks, available_agents, integrator)
                {
                    let mut fallback = fallback_tasks(
                        template,
                        mission_kind,
                        root_prompt,
                        available_agents,
                        Some(&issue),
                        integrator_hint,
                    );
                    fallback.warnings.push(format!(
                        "Planner did not produce a usable bulk plan; using built-in bulk workflow. Reason: {issue}"
                    ));
                    return fallback;
                }
            }
            return parsed;
        }
    }

    if matches!(template, SwarmTemplate::Bulk) {
        let mut fallback = fallback_tasks(
            template,
            mission_kind,
            root_prompt,
            available_agents,
            Some("Planner did not return a valid v2 bulk plan."),
            integrator_hint,
        );
        fallback.warnings.push(
            "Bulk template requires the v2 JSON schema (with deps); using built-in bulk workflow."
                .into(),
        );
        return fallback;
    }

    let Ok(plan) = serde_json::from_str::<SwarmPlanV1>(&json) else {
        return fallback_tasks(
            template,
            mission_kind,
            root_prompt,
            available_agents,
            None,
            integrator_hint,
        );
    };
    let mut tasks = Vec::new();
    let mut idx = 0usize;
    let mut seen_agents = HashSet::new();
    for task in plan.tasks.into_iter() {
        let agent_id = task.agent_id.trim().to_string();
        if agent_id.is_empty() {
            continue;
        }
        if available_agents.iter().all(|id| id != &agent_id) {
            continue;
        }
        // Keep v1 deterministic: at most one task per agent id.
        if !seen_agents.insert(agent_id.clone()) {
            continue;
        }
        let title = task.title.trim().to_string();
        let prompt = task.prompt.trim().to_string();
        if title.is_empty() || prompt.is_empty() {
            continue;
        }
        idx = idx.saturating_add(1);
        tasks.push(SwarmTask {
            id: format!("task-{idx:02}"),
            agent_id,
            role: None,
            title,
            task_prompt: prompt,
            deps: Vec::new(),
            writes: false,
            artifacts: Vec::new(),
            done_when: None,
            state: SwarmTaskState::Pending,
            output: None,
            parsed_artifacts: None,
            expected_artifacts_missing: false,
            failed: false,
            retries: 0,
        });
    }

    if tasks.is_empty() {
        return fallback_tasks(
            template,
            mission_kind,
            root_prompt,
            available_agents,
            None,
            integrator_hint,
        );
    }
    tasks.truncate(available_agents.len());

    ParsedSwarmPlan {
        tasks,
        synthesis_prompt: plan.synthesis_prompt,
        integrator_agent_id: None,
        warnings: Vec::new(),
    }
}

fn parse_v2_plan(
    plan: SwarmPlanV2,
    template: SwarmTemplate,
    available_agents: &[String],
    integrator_hint: Option<&str>,
    integrator_locked: bool,
    multi_integrator: bool,
) -> Option<ParsedSwarmPlan> {
    if plan.tasks.is_empty() {
        return None;
    }
    if let Some(version) = plan.version {
        if version != 2 {
            return None;
        }
    }

    let integrator_plan = plan
        .integrator_agent_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let integrator_hint = integrator_hint.map(str::trim).filter(|id| !id.is_empty());
    let integrator_locked = integrator_locked && integrator_hint.is_some();

    let mut warnings = Vec::new();
    if integrator_locked {
        if let (Some(plan_id), Some(hint_id)) = (integrator_plan, integrator_hint) {
            if !plan_id.eq_ignore_ascii_case(hint_id) {
                warnings.push(format!(
                    "Planner returned integrator_agent_id '{plan_id}' but integrator is locked to '{hint_id}'; ignoring planner override."
                ));
            }
        }
    }
    let inferred_integrator = if integrator_locked || integrator_plan.is_some() {
        None
    } else {
        infer_integrator_agent_id_from_v2_tasks(plan.tasks.as_slice(), available_agents)
    };
    if let Some((agent_id, reason)) = inferred_integrator.as_ref() {
        warnings.push(format!(
            "Planner omitted integrator_agent_id; inferred integrator '{agent_id}' from {reason}."
        ));
    }

    let integrator_candidate = if integrator_locked {
        integrator_hint
    } else {
        integrator_plan
            .or(inferred_integrator
                .as_ref()
                .map(|(agent_id, _)| agent_id.as_str()))
            .or(integrator_hint)
    };
    let integrator = integrator_candidate.and_then(|id| {
        available_agents
            .iter()
            .find(|candidate| candidate.as_str() == id)
            .map(|id| id.to_string())
    });
    if let Some(plan_template) = plan
        .template
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let plan_template = parse_swarm_template(Some(plan_template));
        if plan_template != template {
            warnings.push(format!(
                "Planner returned template '{}' but swarm is running template '{}'; continuing with the swarm template.",
                plan_template.label(),
                template.label()
            ));
        }
    }
    let mut tasks = Vec::new();
    let mut seen_ids = HashSet::new();
    for (idx, task) in plan.tasks.into_iter().enumerate() {
        let agent_id = task.agent_id.trim().to_string();
        if agent_id.is_empty() {
            continue;
        }
        if available_agents.iter().all(|id| id != &agent_id) {
            continue;
        }

        let title = task.title.trim().to_string();
        let prompt = task.prompt.trim().to_string();
        if title.is_empty() || prompt.is_empty() {
            continue;
        }

        let id = task
            .id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("task-{:02}", idx + 1));
        if !seen_ids.insert(id.clone()) {
            warnings.push(format!(
                "Duplicate task id '{id}' in planner output; skipping."
            ));
            continue;
        }

        let mut writes = task.writes;
        if writes && !multi_integrator {
            let allowed = integrator
                .as_deref()
                .is_some_and(|integrator| integrator == agent_id.as_str());
            if !allowed {
                writes = false;
                warnings.push(format!(
                    "Planner marked task '{id}' as writes=true but agent '{agent_id}' is not the integrator; forcing read-only."
                ));
            }
        }

        let deps = task
            .deps
            .into_iter()
            .map(|dep| dep.trim().to_string())
            .filter(|dep| !dep.is_empty() && dep != &id)
            .collect::<Vec<_>>();
        // Write-role tasks (integrate) produce file modifications as output —
        // don't declare artifacts for them. Declaring artifacts injects a
        // STRUCTURED ARTIFACTS section into the prompt that forces the agent to
        // produce a JSON block instead of focusing on code edits, and when it
        // doesn't, downstream tasks see a misleading "artifacts missing" error.
        let artifacts = if writes {
            Vec::new()
        } else {
            task.artifacts
                .into_iter()
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .collect::<Vec<_>>()
        };

        tasks.push(SwarmTask {
            id,
            agent_id,
            role: task
                .role
                .map(|r| r.trim().to_string())
                .filter(|r| !r.is_empty()),
            title,
            task_prompt: prompt,
            deps,
            writes,
            artifacts,
            done_when: task
                .done_when
                .map(|d| d.trim().to_string())
                .filter(|d| !d.is_empty()),
            state: SwarmTaskState::Pending,
            output: None,
            parsed_artifacts: None,
            expected_artifacts_missing: false,
            failed: false,
            retries: 0,
        });
    }

    if tasks.is_empty() {
        return None;
    }

    Some(ParsedSwarmPlan {
        tasks,
        synthesis_prompt: plan.synthesis_prompt,
        integrator_agent_id: integrator,
        warnings,
    })
}

fn fallback_tasks(
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    _root_prompt: &str,
    available_agents: &[String],
    plan_error: Option<&str>,
    integrator_hint: Option<&str>,
) -> ParsedSwarmPlan {
    if matches!(template, SwarmTemplate::Bulk) {
        let integrator = integrator_hint
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .and_then(|id| {
                available_agents
                    .iter()
                    .find(|candidate| candidate.as_str() == id)
                    .cloned()
            })
            .or_else(|| available_agents.first().cloned());
        let judge_agent = available_agents
            .iter()
            .find(|id| integrator.as_ref() != Some(*id))
            .cloned()
            .or_else(|| integrator.clone());
        let review_agent = available_agents
            .iter()
            .find(|id| integrator.as_ref() != Some(*id) && judge_agent.as_ref() != Some(*id))
            .cloned()
            .or_else(|| judge_agent.clone())
            .or_else(|| integrator.clone());

        let mut proposer_ids = available_agents
            .iter()
            .filter(|id| integrator.as_ref() != Some(*id))
            .filter(|id| judge_agent.as_ref() != Some(*id))
            .cloned()
            .collect::<Vec<_>>();
        if proposer_ids.is_empty() {
            if let Some(judge) = judge_agent.clone() {
                proposer_ids.push(judge);
            } else if let Some(integrator) = integrator.clone() {
                proposer_ids.push(integrator);
            }
        }
        proposer_ids.truncate(8);

        let proposer_lenses = [
            "minimal diff / safest change",
            "correctness & edge cases",
            "UX/TUI clarity",
            "performance & scalability",
            "testing & verification",
            "docs & maintainability",
            "security & failure modes",
        ];

        let mut tasks = Vec::new();
        let mut proposer_task_ids = Vec::new();
        for (idx, agent_id) in proposer_ids.into_iter().enumerate() {
            let id = format!("propose-{:02}", idx + 1);
            let lens = proposer_lenses
                .get(idx)
                .copied()
                .unwrap_or("alternative approach");
            proposer_task_ids.push(id.clone());
            tasks.push(SwarmTask {
                id,
                agent_id,
                role: Some("propose".into()),
                title: format!("Proposal ({lens})"),
                task_prompt: format!(
                    "Propose an end-to-end solution candidate.\n\nLens: {lens}\n\nConstraints:\n- Do NOT edit the workspace (read-only).\n- Be concrete: file paths, key symbols, and exact commands.\n- If helpful, include a small unified diff (but do not apply it).\n"
                ),
                deps: Vec::new(),
                writes: false,
                artifacts: vec!["options".into(), "files".into(), "commands".into(), "risks".into()],
                done_when: Some(
                    "We have a concrete, repo-grounded candidate solution with tradeoffs."
                        .into(),
                ),
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }

        if let Some(agent_id) = judge_agent.clone() {
            tasks.push(SwarmTask {
                id: "judge".into(),
                agent_id,
                role: Some("judge".into()),
                title: "Judge + select approach".into(),
                task_prompt: "Compare the proposer outputs and pick the best approach. Provide:\n- Decision (which proposal / why)\n- A step-by-step integration plan for the integrator\n- Acceptance criteria\n- Exact verification commands\n\nConstraints:\n- Do NOT edit the workspace (read-only).\n"
                    .into(),
                deps: proposer_task_ids.clone(),
                writes: false,
                artifacts: vec!["decision".into(), "plan".into(), "commands".into(), "risks".into()],
                done_when: Some("Integrator has a clear, actionable plan to implement.".into()),
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }

        if let Some(agent_id) = integrator.clone() {
            tasks.push(SwarmTask {
                id: "integrate".into(),
                agent_id,
                role: Some("integrate".into()),
                title: "Integrate selected approach".into(),
                task_prompt: "Implement the selected approach using the judge output.\n\nConstraints:\n- You are the ONLY agent allowed to edit the workspace.\n- Prefer small, safe diffs.\n- Run the suggested verification commands.\n"
                    .into(),
                deps: vec!["judge".into()],
                writes: true,
                artifacts: Vec::new(),
                done_when: Some("Changes are implemented cleanly with validations passing.".into()),
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }

        if let Some(agent_id) = review_agent {
            tasks.push(SwarmTask {
                id: "review".into(),
                agent_id,
                role: Some("review".into()),
                title: "Review final diff".into(),
                task_prompt: "Review the integrated changes for correctness, UX, and maintainability. Suggest follow-ups and edge cases.\n\nConstraints:\n- Do NOT edit the workspace (read-only).\n"
                    .into(),
                deps: vec!["integrate".into()],
                writes: false,
                artifacts: vec!["risks".into(), "commands".into()],
                done_when: Some("We have confidence in correctness and know remaining risks.".into()),
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }

        let synth = plan_error.map(|err| {
            format!(
                "Note: planner output could not be used; fallback prompts were used. Reason: {err}"
            )
        });

        return ParsedSwarmPlan {
            tasks,
            synthesis_prompt: synth,
            integrator_agent_id: integrator,
            warnings: Vec::new(),
        };
    }
    if matches!(template, SwarmTemplate::Lab) {
        let integrator = integrator_hint
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .and_then(|id| {
                available_agents
                    .iter()
                    .find(|candidate| candidate.as_str() == id)
                    .cloned()
            })
            .or_else(|| available_agents.first().cloned());
        let recon_agent = available_agents
            .iter()
            .find(|id| integrator.as_ref() != Some(*id))
            .cloned()
            .or_else(|| integrator.clone());
        let design_agent = available_agents
            .iter()
            .find(|id| integrator.as_ref() != Some(*id) && recon_agent.as_ref() != Some(*id))
            .cloned()
            .or_else(|| recon_agent.clone())
            .or_else(|| integrator.clone());
        let review_agent = available_agents
            .iter()
            .find(|id| {
                integrator.as_ref() != Some(*id)
                    && recon_agent.as_ref() != Some(*id)
                    && design_agent.as_ref() != Some(*id)
            })
            .cloned()
            .or_else(|| design_agent.clone())
            .or_else(|| recon_agent.clone())
            .or_else(|| integrator.clone());

        let mut tasks = Vec::new();
        let research_mission = mission_kind.allows_research_roles();
        if let Some(agent_id) = recon_agent {
            let (role, title, task_prompt, artifacts, done_when) = match mission_kind {
                SwarmMissionKind::Research => (
                    Some("research".into()),
                    "Sources + prior-art survey".into(),
                    "Survey papers, docs, web resources, and related references for the operator request. Extract the strongest sources, competing ideas, and the key assumptions or unknowns. Stay read-only and keep the output grounded in evidence.".into(),
                    vec!["sources".into(), "notes".into(), "risks".into()],
                    Some("We have a grounded map of the best sources, references, and research directions.".into()),
                ),
                SwarmMissionKind::ComputationalResearch => (
                    Some("research".into()),
                    "Sources + problem framing".into(),
                    "Survey papers, docs, datasets, and web resources to frame the problem. Summarize the strongest prior work, data sources, evaluation criteria, and the assumptions the computational lane should test. Stay read-only.".into(),
                    vec!["sources".into(), "methods".into(), "risks".into()],
                    Some("We have a solid source base and a clear problem framing for computational work.".into()),
                ),
                SwarmMissionKind::General => (
                    None,
                    "Codebase recon".into(),
                    "Scan the repository for the most relevant files/modules and summarize where changes should happen. Provide concrete file paths and key functions/symbols. Avoid proposing large diffs; focus on mapping the terrain and risks.".into(),
                    vec!["files".into(), "risks".into()],
                    Some("We know exactly where changes should happen and the main risks.".into()),
                ),
            };
            tasks.push(SwarmTask {
                id: "recon".into(),
                agent_id,
                role,
                title,
                task_prompt,
                deps: Vec::new(),
                writes: false,
                artifacts,
                done_when,
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }
        if let Some(agent_id) = design_agent {
            let (role, title, task_prompt, artifacts, done_when) = match mission_kind {
                SwarmMissionKind::Research => (
                    Some("research".into()),
                    "Compare directions + rank strategies".into(),
                    "Use the strongest sources to compare competing ideas, strategies, or solution paths. Rank the best options, explain tradeoffs, and call out what still needs validation. Stay read-only.".into(),
                    vec!["sources".into(), "methods".into(), "options".into()],
                    Some("We have ranked strategy options with evidence, tradeoffs, and open questions.".into()),
                ),
                SwarmMissionKind::ComputationalResearch => (
                    Some(COMPUTATIONAL_RESEARCH_ROLE.into()),
                    "Model + evaluate candidates".into(),
                    "Run the computation-heavy lane: use calculations, simulations, modeling, numerical methods, experiments, optimization, or reproducible analysis when helpful. Compare candidate strategies quantitatively, explain methods, and surface assumptions or data gaps. Stay read-only.".into(),
                    vec!["methods".into(), "options".into(), "commands".into()],
                    Some("We have a computationally grounded ranking of candidate strategies and the methods behind it.".into()),
                ),
                SwarmMissionKind::General => (
                    Some("propose".into()),
                    "Design options".into(),
                    "Propose 2-3 plausible implementation approaches (with tradeoffs) and call out which files/modules each approach touches. Keep it specific and repo-grounded.".into(),
                    vec!["options".into(), "files".into()],
                    Some("We have 1-2 clear, repo-grounded approaches with tradeoffs.".into()),
                ),
            };
            tasks.push(SwarmTask {
                id: "design".into(),
                agent_id,
                role,
                title,
                task_prompt,
                deps: Vec::new(),
                writes: false,
                artifacts,
                done_when,
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }
        if let Some(agent_id) = integrator.clone() {
            let (title, task_prompt, writes, artifacts, done_when) = match mission_kind {
                SwarmMissionKind::Research => (
                    "Synthesize findings + recommendation".into(),
                    "Integrate the upstream research into a decisive recommendation for the operator. Produce a concise synthesis, ranked next steps, and any follow-up research gaps. Stay read-only unless the operator explicitly asked for repo changes.".into(),
                    false,
                    vec!["notes".into(), "sources".into(), "commands".into()],
                    Some("We have a clear recommendation backed by sources, assumptions, and ranked follow-ups.".into()),
                ),
                SwarmMissionKind::ComputationalResearch => (
                    "Synthesize evidence + next-step plan".into(),
                    "Integrate the upstream source survey and computational analysis into a decisive recommendation. Summarize the strongest evidence, methods, assumptions, ranked next steps, and any follow-up experiments. Stay read-only unless the operator explicitly asked for repo changes.".into(),
                    false,
                    vec!["notes".into(), "methods".into(), "commands".into()],
                    Some("We have a computationally grounded recommendation with methods, assumptions, and next experiments.".into()),
                ),
                SwarmMissionKind::General => (
                    "Integrate + implement".into(),
                    "Implement the best approach using the dependency outputs. You are the only agent allowed to make workspace edits in this swarm. Prefer small, safe diffs and keep tests green.".into(),
                    true,
                    Vec::new(),
                    Some("Changes are implemented cleanly with validations to run.".into()),
                ),
            };
            tasks.push(SwarmTask {
                id: "implement".into(),
                agent_id,
                role: Some("integrate".into()),
                title,
                task_prompt,
                deps: vec!["recon".into(), "design".into()],
                writes,
                artifacts,
                done_when,
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }
        if let Some(agent_id) = review_agent {
            let (task_prompt, artifacts, done_when) = if research_mission {
                (
                    "Review the synthesized findings for weak evidence, missing sources, shaky assumptions, and overlooked follow-up questions. Suggest better references, validation steps, or experiments as text only; do not apply changes.".into(),
                    vec!["risks".into(), "sources".into(), "commands".into()],
                    Some("We know the main evidence gaps, risks, and the next checks to run.".into()),
                )
            } else {
                (
                    "Review the implemented approach for correctness, UX, and maintainability. Suggest verification steps (exact commands) and edge cases. If you propose edits, do so as text/diff; do not apply changes.".into(),
                    vec!["risks".into(), "commands".into()],
                    Some("We have confidence in correctness and a clear test plan.".into()),
                )
            };
            tasks.push(SwarmTask {
                id: "review".into(),
                agent_id,
                role: Some("review".into()),
                title: "Review & verification".into(),
                task_prompt,
                deps: vec!["implement".into()],
                writes: false,
                artifacts,
                done_when,
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }

        let synth = plan_error.map(|err| {
            format!(
                "Note: planner output could not be used; fallback prompts were used. Reason: {err}"
            )
        });

        return ParsedSwarmPlan {
            tasks,
            synthesis_prompt: synth,
            integrator_agent_id: integrator,
            warnings: Vec::new(),
        };
    }

    let mut tasks = Vec::new();
    let mut idx = 0usize;
    for (agent_idx, agent_id) in available_agents.iter().enumerate() {
        idx = idx.saturating_add(1);
        let (role, title, prompt, deps, writes) = match mission_kind {
            SwarmMissionKind::Research => match agent_idx {
                0 => (
                    Some("research".to_string()),
                    "Source survey",
                    "Survey papers, docs, web resources, and related references. Identify the strongest sources, competing ideas, and missing information.",
                    Vec::new(),
                    false,
                ),
                1 => (
                    Some("research".to_string()),
                    "Strategy comparison",
                    "Compare the best ideas from the sources, rank the strongest strategies, and explain their tradeoffs, assumptions, and open questions.",
                    Vec::new(),
                    false,
                ),
                2 => (
                    Some("review".to_string()),
                    "Gap review",
                    "Review the research outputs for weak evidence, missing sources, shaky assumptions, and better follow-up directions.",
                    Vec::new(),
                    false,
                ),
                _ => (
                    Some("review".to_string()),
                    "Review & pitfalls",
                    "Review the proposed research direction for evidence quality, missing citations, and strategic blind spots.",
                    Vec::new(),
                    false,
                ),
            },
            SwarmMissionKind::ComputationalResearch => match agent_idx {
                0 => (
                    Some("research".to_string()),
                    "Source survey",
                    "Survey papers, docs, datasets, and related resources. Summarize prior work, evaluation criteria, and the most useful evidence for downstream computational analysis.",
                    Vec::new(),
                    false,
                ),
                1 => (
                    Some(COMPUTATIONAL_RESEARCH_ROLE.to_string()),
                    "Model + experiment lane",
                    "Use simulations, modeling, numerical methods, optimization, calculations, or reproducible analysis when helpful. Compare candidate strategies and explain methods, commands, and assumptions.",
                    Vec::new(),
                    false,
                ),
                2 => (
                    Some("review".to_string()),
                    "Evidence review",
                    "Review the research and computational outputs for weak methods, missing baselines, data issues, and follow-up experiments.",
                    Vec::new(),
                    false,
                ),
                _ => (
                    Some("review".to_string()),
                    "Review & pitfalls",
                    "Review the proposed computational research direction for evidence quality, methodological risks, and better alternatives.",
                    Vec::new(),
                    false,
                ),
            },
            SwarmMissionKind::General => match (template, agent_idx) {
                (SwarmTemplate::Lab, 0) => (
                    Some("research".to_string()),
                    "Codebase recon",
                    "Scan the repository for the most relevant files/modules and summarize where changes should happen. Provide concrete file paths and key functions/symbols. Avoid proposing large diffs; focus on mapping the terrain and risks.",
                    Vec::new(),
                    false,
                ),
                (SwarmTemplate::Lab, 1) => (
                    Some("research".to_string()),
                    "Design options",
                    "Propose 2-3 plausible implementation approaches (with tradeoffs) and call out which files/modules each approach touches. Keep it specific and repo-grounded.",
                    Vec::new(),
                    false,
                ),
                (SwarmTemplate::Lab, 2) => (
                    Some("integrate".to_string()),
                    "Integrate + implement",
                    "Implement the best approach using the dependency outputs. You are the only agent allowed to make workspace edits in this swarm. Prefer small, safe diffs and keep tests green.",
                    vec!["task-01".into(), "task-02".into()],
                    true,
                ),
                (SwarmTemplate::Lab, _) => (
                    Some("review".to_string()),
                    "Review & verification",
                    "Review the proposed approach for correctness, UX, and maintainability. Suggest verification steps (exact commands) and edge cases. If you propose edits, do so as text/diff; do not apply changes.",
                    vec!["task-03".into()],
                    false,
                ),
                (_, 0) => (
                    Some("recon".to_string()),
                    "Codebase recon",
                    "Scan the repository for the most relevant files/modules and summarize where changes should happen. Provide concrete file paths and key functions/symbols. Avoid proposing large diffs; focus on mapping the terrain and risks.",
                    Vec::new(),
                    false,
                ),
                (_, 1) => (
                    Some("plan".to_string()),
                    "Implementation plan",
                    "Propose an implementation approach and the specific code changes needed. If appropriate, provide a concise unified diff for the most important edits. Call out any concurrency/file-conflict risks with multiple agents.",
                    Vec::new(),
                    false,
                ),
                (_, 2) => (
                    Some("test".to_string()),
                    "Tests & verification",
                    "Propose how to verify the change (tests, manual checks, edge cases). If tests likely exist, suggest exact commands and where to add/update test coverage.",
                    Vec::new(),
                    false,
                ),
                (_, _) => (
                    Some("review".to_string()),
                    "Review & pitfalls",
                    "Review the planned approach for correctness, UX, and maintainability. Point out edge cases, failure modes, and simpler alternatives.",
                    Vec::new(),
                    false,
                ),
            },
        };

        let task_id = format!("task-{idx:02}");
        tasks.push(SwarmTask {
            id: task_id,
            agent_id: agent_id.clone(),
            role,
            title: title.into(),
            task_prompt: prompt.into(),
            deps,
            writes,
            artifacts: Vec::new(),
            done_when: None,
            state: SwarmTaskState::Pending,
            output: None,
            parsed_artifacts: None,
            expected_artifacts_missing: false,
            failed: false,
            retries: 0,
        });
    }

    let synth = plan_error.map(|err| {
        format!("Note: planner output could not be used; fallback prompts were used. Reason: {err}")
    });

    ParsedSwarmPlan {
        tasks,
        synthesis_prompt: synth,
        integrator_agent_id: None,
        warnings: Vec::new(),
    }
}

#[derive(Clone, Debug, Default)]
struct SwarmDagIssues {
    unknown_deps: Vec<(String, String)>,
    cycle: Option<Vec<String>>,
}

impl SwarmDagIssues {
    fn is_empty(&self) -> bool {
        self.unknown_deps.is_empty() && self.cycle.is_none()
    }

    fn summary(&self) -> String {
        let mut parts = Vec::new();

        if !self.unknown_deps.is_empty() {
            let mut examples = self
                .unknown_deps
                .iter()
                .take(6)
                .map(|(task, dep)| format!("{task}->{dep}"))
                .collect::<Vec<_>>();
            if self.unknown_deps.len() > examples.len() {
                examples.push("…".into());
            }
            parts.push(format!(
                "unknown deps: {} ({} total)",
                examples.join(", "),
                self.unknown_deps.len()
            ));
        }

        if let Some(cycle) = self.cycle.as_ref() {
            let mut items = cycle.clone();
            if items.len() > 12 {
                items.truncate(12);
                items.push("…".into());
            }
            parts.push(format!("cycle: {}", items.join(" -> ")));
        }

        if parts.is_empty() {
            "ok".into()
        } else {
            parts.join("; ")
        }
    }
}

fn analyze_swarm_dag(tasks: &[SwarmTask]) -> SwarmDagIssues {
    let mut issues = SwarmDagIssues::default();
    if tasks.is_empty() {
        return issues;
    }

    let ids = tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<HashSet<_>>();
    for task in tasks.iter() {
        for dep in task.deps.iter() {
            if !ids.contains(dep.as_str()) {
                issues.unknown_deps.push((task.id.clone(), dep.clone()));
            }
        }
    }

    issues.cycle = find_swarm_cycle_path(tasks);
    issues
}

fn find_swarm_cycle_path(tasks: &[SwarmTask]) -> Option<Vec<String>> {
    if tasks.is_empty() {
        return None;
    }
    let idx_by_id = tasks
        .iter()
        .enumerate()
        .map(|(idx, task)| (task.id.as_str(), idx))
        .collect::<HashMap<_, _>>();
    let mut state = vec![0u8; tasks.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut on_stack = vec![false; tasks.len()];

    fn dfs(
        v: usize,
        tasks: &[SwarmTask],
        idx_by_id: &HashMap<&str, usize>,
        state: &mut [u8],
        stack: &mut Vec<usize>,
        on_stack: &mut [bool],
    ) -> Option<Vec<String>> {
        state[v] = 1;
        stack.push(v);
        on_stack[v] = true;

        for dep in tasks[v].deps.iter() {
            let Some(&u) = idx_by_id.get(dep.as_str()) else {
                continue;
            };
            if state[u] == 0 {
                if let Some(cycle) = dfs(u, tasks, idx_by_id, state, stack, on_stack) {
                    return Some(cycle);
                }
            } else if on_stack[u] {
                let Some(pos) = stack.iter().position(|&idx| idx == u) else {
                    continue;
                };
                let mut cycle = stack[pos..]
                    .iter()
                    .map(|&idx| tasks[idx].id.clone())
                    .collect::<Vec<_>>();
                cycle.push(tasks[u].id.clone());
                return Some(cycle);
            }
        }

        stack.pop();
        on_stack[v] = false;
        state[v] = 2;
        None
    }

    for v in 0..tasks.len() {
        if state[v] != 0 {
            continue;
        }
        if let Some(cycle) = dfs(v, tasks, &idx_by_id, &mut state, &mut stack, &mut on_stack) {
            return Some(cycle);
        }
    }

    None
}

fn find_swarm_cycle_back_edge(tasks: &[SwarmTask]) -> Option<(usize, String)> {
    if tasks.is_empty() {
        return None;
    }
    let idx_by_id = tasks
        .iter()
        .enumerate()
        .map(|(idx, task)| (task.id.as_str(), idx))
        .collect::<HashMap<_, _>>();
    let mut state = vec![0u8; tasks.len()];
    let mut on_stack = vec![false; tasks.len()];

    fn dfs(
        v: usize,
        tasks: &[SwarmTask],
        idx_by_id: &HashMap<&str, usize>,
        state: &mut [u8],
        on_stack: &mut [bool],
    ) -> Option<(usize, String)> {
        state[v] = 1;
        on_stack[v] = true;

        for dep in tasks[v].deps.iter() {
            let Some(&u) = idx_by_id.get(dep.as_str()) else {
                continue;
            };
            if state[u] == 0 {
                if let Some(edge) = dfs(u, tasks, idx_by_id, state, on_stack) {
                    return Some(edge);
                }
            } else if on_stack[u] {
                return Some((v, dep.clone()));
            }
        }

        on_stack[v] = false;
        state[v] = 2;
        None
    }

    for v in 0..tasks.len() {
        if state[v] != 0 {
            continue;
        }
        if let Some(edge) = dfs(v, tasks, &idx_by_id, &mut state, &mut on_stack) {
            return Some(edge);
        }
    }
    None
}

fn repair_swarm_dag(tasks: &mut [SwarmTask]) -> Vec<String> {
    if tasks.is_empty() {
        return Vec::new();
    }

    let ids = tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<HashSet<_>>();

    let mut removed_unknown_total = 0usize;
    let mut removed_unknown_examples: Vec<(String, String)> = Vec::new();
    let mut removed_dupe_total = 0usize;
    for task in tasks.iter_mut() {
        let mut seen: HashSet<String> = HashSet::new();
        task.deps.retain(|dep| {
            if dep == &task.id {
                return false;
            }
            if !ids.contains(dep) {
                removed_unknown_total = removed_unknown_total.saturating_add(1);
                if removed_unknown_examples.len() < 6 {
                    removed_unknown_examples.push((task.id.clone(), dep.clone()));
                }
                return false;
            }
            if !seen.insert(dep.clone()) {
                removed_dupe_total = removed_dupe_total.saturating_add(1);
                return false;
            }
            true
        });
    }

    let mut removed_cycle_total = 0usize;
    let mut removed_cycle_examples: Vec<(String, String)> = Vec::new();
    while let Some((task_idx, dep_id)) = find_swarm_cycle_back_edge(tasks) {
        let Some(pos) = tasks[task_idx].deps.iter().position(|dep| dep == &dep_id) else {
            break;
        };
        tasks[task_idx].deps.remove(pos);
        removed_cycle_total = removed_cycle_total.saturating_add(1);
        if removed_cycle_examples.len() < 6 {
            removed_cycle_examples.push((tasks[task_idx].id.clone(), dep_id));
        }
    }

    let mut warnings = Vec::new();
    if removed_unknown_total > 0 {
        let examples = removed_unknown_examples
            .into_iter()
            .map(|(task, dep)| format!("{task}->{dep}"))
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!(
            "DAG repair: removed {removed_unknown_total} unknown dep(s){}",
            if examples.is_empty() {
                ".".into()
            } else {
                format!(" (examples: {examples}).")
            }
        ));
    }
    if removed_dupe_total > 0 {
        warnings.push(format!(
            "DAG repair: removed {removed_dupe_total} duplicate dep(s)."
        ));
    }
    if removed_cycle_total > 0 {
        let examples = removed_cycle_examples
            .into_iter()
            .map(|(task, dep)| format!("{task}->{dep}"))
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!(
            "DAG repair: removed {removed_cycle_total} dep(s) to break cycle(s){}",
            if examples.is_empty() {
                ".".into()
            } else {
                format!(" (examples: {examples}).")
            }
        ));
    }

    warnings
}

fn initialize_task_graph(run: &mut SwarmRun) {
    let ids = run
        .tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<HashSet<_>>();
    for task in run.tasks.iter_mut() {
        let mut seen = HashSet::new();
        task.deps.retain(|dep| {
            if dep == &task.id {
                return false;
            }
            if !ids.contains(dep) {
                return false;
            }
            seen.insert(dep.clone())
        });
        if task.state.is_terminal() {
            continue;
        }
        task.state = if task.deps.is_empty() {
            SwarmTaskState::Ready
        } else {
            SwarmTaskState::Pending
        };
    }
}

fn tasks_terminal_count(tasks: &[SwarmTask]) -> usize {
    tasks.iter().filter(|task| task.state.is_terminal()).count()
}

fn mark_task_running(run: &mut SwarmRun, agent_id: &str) {
    if !matches!(run.stage, SwarmStage::Executing) {
        return;
    }
    if let Some(task) = run
        .tasks
        .iter_mut()
        .find(|task| task.agent_id == agent_id && matches!(task.state, SwarmTaskState::Dispatched))
    {
        task.state = SwarmTaskState::Running;
    }
}

struct TaskCompletion {
    task_id: String,
    expected_artifacts_missing: bool,
    /// Task declared `writes: true` — it was supposed to modify files.
    writes_expected: bool,
    /// At least one file was attributed to this agent via `FileWrite` events.
    writes_detected: bool,
}

fn mark_task_finished(
    run: &mut SwarmRun,
    agent_id: &str,
    message: String,
    failed: bool,
    agent_has_file_writes: bool,
) -> Option<TaskCompletion> {
    // Look for an active (Running or Dispatched) task first.
    let pos_active = run
        .tasks
        .iter()
        .position(|task| task.agent_id == agent_id && matches!(task.state, SwarmTaskState::Running))
        .or_else(|| {
            run.tasks.iter().position(|task| {
                task.agent_id == agent_id && matches!(task.state, SwarmTaskState::Dispatched)
            })
        });

    // Fall back to an already-finished task so late/intermediate responses
    // can still append artifacts.
    let pos_finished = || {
        run.tasks.iter().position(|task| {
            task.agent_id == agent_id
                && matches!(task.state, SwarmTaskState::Done | SwarmTaskState::Failed)
        })
    };
    let pos = pos_active.or_else(pos_finished)?;
    let already_finished = pos_active.is_none();

    let parsed_artifacts = parse_task_artifacts(&run.tasks[pos].id, &message);
    // Write-role tasks (integrate) produce file modifications as their primary
    // output — the structured artifacts JSON is optional metadata.  Only flag
    // missing artifacts for read-only tasks where the JSON is the sole output.
    let expected_artifacts_missing = !run.tasks[pos].artifacts.is_empty()
        && parsed_artifacts.is_none()
        && !run.tasks[pos].writes;

    let task = &mut run.tasks[pos];

    if already_finished {
        // Append output to existing response.
        if let Some(existing) = task.output.as_mut() {
            existing.push_str("\n\n---\n\n");
            existing.push_str(&message);
        } else {
            task.output = Some(message);
        }
    } else {
        task.output = Some(message);
        task.failed = failed;
        task.state = if failed {
            SwarmTaskState::Failed
        } else {
            SwarmTaskState::Done
        };
        task.expected_artifacts_missing = expected_artifacts_missing;
    }

    // Merge artifacts with any previously collected ones instead of overwriting.
    match (task.parsed_artifacts.as_mut(), parsed_artifacts) {
        (Some(existing), Some(new)) => merge_task_artifacts(existing, new),
        (None, new @ Some(_)) => task.parsed_artifacts = new,
        _ => {}
    }

    let writes_expected = task.writes;

    Some(TaskCompletion {
        task_id: task.id.clone(),
        expected_artifacts_missing: if already_finished {
            false
        } else {
            expected_artifacts_missing
        },
        writes_expected,
        writes_detected: agent_has_file_writes,
    })
}

fn refresh_task_readiness(run: &mut SwarmRun) {
    let all_ids = run
        .tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<HashSet<_>>();
    let terminal_ids = run
        .tasks
        .iter()
        .filter(|task| task.state.is_terminal())
        .map(|task| task.id.clone())
        .collect::<HashSet<_>>();
    for task in run.tasks.iter_mut() {
        if !matches!(task.state, SwarmTaskState::Pending) {
            continue;
        }
        let ready = task.deps.iter().all(|dep| {
            terminal_ids
                .contains(dep)
                // Unknown dep shouldn't happen after sanitize; treat as satisfied to avoid deadlocks.
                || !all_ids.contains(dep)
        });
        if ready {
            task.state = SwarmTaskState::Ready;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SwarmDeadlock {
    skipped: Vec<String>,
    message: String,
}

fn maybe_resolve_deadlock(run: &mut SwarmRun) -> Option<SwarmDeadlock> {
    let has_active_or_ready = run.tasks.iter().any(|task| {
        matches!(
            task.state,
            SwarmTaskState::Ready | SwarmTaskState::Dispatched | SwarmTaskState::Running
        )
    });
    if has_active_or_ready {
        return None;
    }

    let pending = run
        .tasks
        .iter()
        .filter(|task| matches!(task.state, SwarmTaskState::Pending))
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return None;
    }

    let pending_ids = pending.iter().map(|id| id.as_str()).collect::<HashSet<_>>();
    let pending_tasks = run
        .tasks
        .iter()
        .filter(|task| matches!(task.state, SwarmTaskState::Pending))
        .cloned()
        .collect::<Vec<_>>();

    let mut message = String::new();
    message.push_str(&format!(
        "Swarm deadlock: skipping tasks with unresolvable deps: {}",
        pending.join(", ")
    ));

    if let Some(cycle) = find_swarm_cycle_path(pending_tasks.as_slice()) {
        let mut cycle = cycle;
        if cycle.len() > 12 {
            cycle.truncate(12);
            cycle.push("…".into());
        }
        message.push_str(&format!("\nCycle detected: {}", cycle.join(" -> ")));
    }

    message.push_str("\nBlocked on:");
    for task in pending_tasks.iter().take(12) {
        let deps = task
            .deps
            .iter()
            .filter(|dep| pending_ids.contains(dep.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if deps.is_empty() {
            message.push_str(&format!("\n- {} waits on: (none)", task.id));
        } else {
            message.push_str(&format!("\n- {} waits on: {}", task.id, deps.join(", ")));
        }
    }

    for task in run.tasks.iter_mut() {
        if matches!(task.state, SwarmTaskState::Pending) {
            task.state = SwarmTaskState::Skipped;
            task.failed = true;
            if task.output.is_none() {
                task.output = Some("SKIPPED (unresolvable deps)".into());
            }
        }
    }

    Some(SwarmDeadlock {
        skipped: pending,
        message,
    })
}

fn dispatch_ready_tasks(run: &mut SwarmRun) -> Vec<SwarmDispatch> {
    let indices = select_dispatchable_ready_task_indices(run);
    let mut dispatches = Vec::new();
    for idx in indices {
        let task = &run.tasks[idx];
        let deps_payload = collect_dependency_payload(run, task);
        let prompt = if deps_payload.is_empty() {
            wrap_task_prompt(
                &run.root_prompt,
                run.mission_kind,
                task,
                None,
                &run.scope_files,
            )
        } else {
            wrap_task_prompt(
                &run.root_prompt,
                run.mission_kind,
                task,
                Some(deps_payload.as_slice()),
                &run.scope_files,
            )
        };
        let agent_id = task.agent_id.clone();
        let task_role = task.role.clone();
        run.tasks[idx].state = SwarmTaskState::Dispatched;
        dispatches.push(SwarmDispatch {
            agent_id,
            mission_id: run.mission_id.clone(),
            prompt,
            task_role,
        });
    }
    dispatches
}

fn select_dispatchable_ready_task_indices(run: &SwarmRun) -> Vec<usize> {
    let mut writer_taken = run.tasks.iter().any(|task| {
        task.writes
            && matches!(
                task.state,
                SwarmTaskState::Dispatched | SwarmTaskState::Running
            )
    });
    let mut indices = Vec::new();
    for (idx, task) in run.tasks.iter().enumerate() {
        if !matches!(task.state, SwarmTaskState::Ready) {
            continue;
        }
        if task.writes {
            if writer_taken {
                continue;
            }
            writer_taken = true;
        }
        indices.push(idx);
    }
    indices
}

fn collect_dependency_payload(run: &SwarmRun, task: &SwarmTask) -> Vec<(String, String)> {
    let role = task.role.as_deref().and_then(normalize_role_label);
    // Tasks that must ACT on dependency outputs need the full raw text —
    // compact artifact summaries strip reasoning and implementation details,
    // causing agents to describe changes instead of executing them.
    //
    // Full output for: judge (comparing proposals), integrate (implementing),
    // and any task with `writes: true` (custom write-role tasks from planner).
    let needs_full_output = matches!(role.as_deref(), Some("judge" | "integrate")) || task.writes;
    let max_chars = if needs_full_output {
        SWARM_DEP_OUTPUT_MAX_CHARS_FULL
    } else {
        SWARM_DEP_OUTPUT_MAX_CHARS
    };
    let mut out = Vec::new();
    for dep_id in task.deps.iter() {
        let Some(dep) = run.tasks.iter().find(|t| t.id == *dep_id) else {
            continue;
        };
        let status = match dep.state {
            SwarmTaskState::Done => "DONE",
            SwarmTaskState::Failed => "FAILED",
            SwarmTaskState::Skipped => "SKIPPED",
            _ => "PENDING",
        };
        let label = format!("{} [{}] (agent {})", dep.id, status, dep.agent_id);
        let text = if needs_full_output {
            dependency_payload_text_full(dep)
        } else {
            dependency_payload_text(run, dep)
        };
        out.push((label, truncate_chars(&text, max_chars)));
    }
    out
}

fn dependency_payload_text(run: &SwarmRun, task: &SwarmTask) -> String {
    if let Some(summary) = task_artifacts_summary_for_prompt(task, &run.mission_id) {
        return summary;
    }
    task.output
        .as_deref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "(no output)".into())
}

fn dependency_payload_text_full(task: &SwarmTask) -> String {
    task.output
        .as_deref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "(no output)".into())
}

fn task_artifacts_summary_for_prompt(task: &SwarmTask, mission_id: &str) -> Option<String> {
    let artifacts = task.parsed_artifacts.as_ref()?;
    if artifacts.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    if let Some(summary) = artifacts
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
    {
        lines.push(format!("summary: {summary}"));
    }
    if !artifacts.files.is_empty() {
        let files = artifacts
            .files
            .iter()
            .take(8)
            .map(|entry| match entry.notes.as_deref().map(str::trim) {
                Some(notes) if !notes.is_empty() => format!("{} ({notes})", entry.path),
                _ => entry.path.clone(),
            })
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(format!("files: {files}"));
    }
    if !artifacts.diffs.is_empty() {
        let diffs = artifacts
            .diffs
            .iter()
            .take(8)
            .map(|entry| match entry.path.as_deref().map(str::trim) {
                Some(path) if !path.is_empty() => format!("{path}: {}", entry.summary),
                _ => entry.summary.clone(),
            })
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(format!("diffs: {diffs}"));
    }
    if !artifacts.commands.is_empty() {
        let commands = artifacts
            .commands
            .iter()
            .take(8)
            .map(|entry| match entry.purpose.as_deref().map(str::trim) {
                Some(purpose) if !purpose.is_empty() => format!("{} ({purpose})", entry.cmd),
                _ => entry.cmd.clone(),
            })
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(format!("commands: {commands}"));
    }
    if !artifacts.risks.is_empty() {
        let risks = artifacts
            .risks
            .iter()
            .take(8)
            .map(|entry| {
                let prefix = entry
                    .level
                    .as_deref()
                    .map(str::trim)
                    .filter(|level| !level.is_empty())
                    .map(|level| format!("{level}: "))
                    .unwrap_or_default();
                let mitigation = entry
                    .mitigation
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(|text| format!(" (mitigation: {text})"))
                    .unwrap_or_default();
                format!("{prefix}{}{}", entry.item, mitigation)
            })
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(format!("risks: {risks}"));
    }
    if !artifacts.notes.is_empty() {
        lines.push(format!(
            "notes: {}",
            artifacts
                .notes
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    lines.push(format!(
        "artifact_path: .nit/swarm/{mission_id}/tasks/{}/artifacts.json",
        sanitize_for_filename(&task.id)
    ));
    Some(lines.join("\n"))
}

fn parse_task_artifacts(task_id: &str, message: &str) -> Option<SwarmTaskArtifacts> {
    let mut merged = SwarmTaskArtifacts::default();
    let mut found = false;

    // Primary: look in fenced ```json blocks.
    for json in extract_json_code_blocks(message) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
            continue;
        };
        if let Some(parsed) = parse_task_artifacts_value(task_id, &value) {
            merge_task_artifacts(&mut merged, parsed);
            found = true;
        }
    }

    // Fallback: scan for raw JSON objects containing "swarm_artifacts" in the
    // message body.  Agents sometimes emit the JSON without a code fence, or
    // use a plain ``` fence instead of ```json.
    if !found {
        let text = message.trim();
        let mut search_from = 0;
        while let Some(start) = text[search_from..]
            .find(r#""type":"#)
            .or_else(|| text[search_from..].find(r#""type" :"#))
        {
            let abs_start = search_from + start;
            // Walk backward to find the opening brace.
            let obj_start = match text[..abs_start].rfind('{') {
                Some(s) => s,
                None => {
                    search_from = abs_start + 1;
                    continue;
                }
            };
            // Walk forward to find the matching closing brace.
            let mut depth = 0i32;
            let mut obj_end = None;
            for (i, ch) in text[obj_start..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            obj_end = Some(obj_start + i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end) = obj_end else {
                search_from = abs_start + 1;
                continue;
            };
            let candidate = &text[obj_start..=end];
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(candidate) {
                if let Some(parsed) = parse_task_artifacts_value(task_id, &value) {
                    merge_task_artifacts(&mut merged, parsed);
                    found = true;
                }
            }
            search_from = end + 1;
        }
    }

    if found && !merged.is_empty() {
        Some(merged)
    } else {
        None
    }
}

fn parse_task_artifacts_value(
    task_id: &str,
    value: &serde_json::Value,
) -> Option<SwarmTaskArtifacts> {
    let object = value.as_object()?;
    let typed = object
        .get("type")
        .and_then(|value| value.as_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("swarm_artifacts"));
    let has_artifacts = object.contains_key("artifacts");
    if !typed && !has_artifacts {
        return None;
    }
    if typed
        && object
            .get("version")
            .and_then(|value| value.as_u64())
            .is_some_and(|version| version != 1)
    {
        return None;
    }
    if let Some(owner) = object.get("task_id").and_then(|value| value.as_str()) {
        let owner = owner.trim();
        if !owner.is_empty() && owner != task_id {
            return None;
        }
    }

    let mut parsed = SwarmTaskArtifacts {
        summary: object
            .get("summary")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
            .map(ToString::to_string),
        ..SwarmTaskArtifacts::default()
    };

    let source = object.get("artifacts").unwrap_or(value);
    let source_obj = source.as_object()?;

    parsed.files = parse_artifact_files(source_obj.get("files"));
    parsed.diffs = parse_artifact_diffs(source_obj.get("diffs"));
    parsed.commands = parse_artifact_commands(source_obj.get("commands"));
    parsed.risks = parse_artifact_risks(source_obj.get("risks"));
    parsed.notes = parse_artifact_notes(source_obj.get("notes"));

    if parsed.is_empty() {
        None
    } else {
        Some(parsed)
    }
}

fn parse_artifact_files(value: Option<&serde_json::Value>) -> Vec<SwarmArtifactFile> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(path) = item.as_str().map(str::trim).filter(|path| !path.is_empty()) {
            out.push(SwarmArtifactFile {
                path: path.to_string(),
                notes: None,
            });
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(path) = obj
            .get("path")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        let notes = obj
            .get("notes")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|notes| !notes.is_empty())
            .map(ToString::to_string);
        out.push(SwarmArtifactFile {
            path: path.to_string(),
            notes,
        });
    }
    out
}

fn parse_artifact_diffs(value: Option<&serde_json::Value>) -> Vec<SwarmArtifactDiff> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(summary) = item
            .as_str()
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
        {
            out.push(SwarmArtifactDiff {
                path: None,
                summary: summary.to_string(),
            });
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        let summary = obj
            .get("summary")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|summary| !summary.is_empty());
        let path = obj
            .get("path")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(ToString::to_string);
        let summary = summary.map(ToString::to_string).or_else(|| path.clone());
        let Some(summary) = summary else {
            continue;
        };
        out.push(SwarmArtifactDiff { path, summary });
    }
    out
}

fn parse_artifact_commands(value: Option<&serde_json::Value>) -> Vec<SwarmArtifactCommand> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(cmd) = item.as_str().map(str::trim).filter(|cmd| !cmd.is_empty()) {
            out.push(SwarmArtifactCommand {
                cmd: cmd.to_string(),
                purpose: None,
            });
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(cmd) = obj
            .get("cmd")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|cmd| !cmd.is_empty())
        else {
            continue;
        };
        let purpose = obj
            .get("purpose")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|purpose| !purpose.is_empty())
            .map(ToString::to_string);
        out.push(SwarmArtifactCommand {
            cmd: cmd.to_string(),
            purpose,
        });
    }
    out
}

fn parse_artifact_risks(value: Option<&serde_json::Value>) -> Vec<SwarmArtifactRisk> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(item_text) = item
            .as_str()
            .map(str::trim)
            .filter(|item_text| !item_text.is_empty())
        {
            out.push(SwarmArtifactRisk {
                level: None,
                item: item_text.to_string(),
                mitigation: None,
            });
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(item_text) = obj
            .get("item")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|item_text| !item_text.is_empty())
        else {
            continue;
        };
        let level = obj
            .get("level")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|level| !level.is_empty())
            .map(ToString::to_string);
        let mitigation = obj
            .get("mitigation")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|mitigation| !mitigation.is_empty())
            .map(ToString::to_string);
        out.push(SwarmArtifactRisk {
            level,
            item: item_text.to_string(),
            mitigation,
        });
    }
    out
}

fn parse_artifact_notes(value: Option<&serde_json::Value>) -> Vec<String> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(note) = item.as_str().map(str::trim).filter(|note| !note.is_empty()) {
            out.push(note.to_string());
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        if let Some(note) = obj
            .get("note")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|note| !note.is_empty())
        {
            out.push(note.to_string());
        }
    }
    out
}

fn merge_task_artifacts(dst: &mut SwarmTaskArtifacts, src: SwarmTaskArtifacts) {
    if let Some(summary) = src
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
    {
        dst.summary = Some(summary.to_string());
    }

    let mut seen_files = dst
        .files
        .iter()
        .map(|entry| entry.path.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    for entry in src.files {
        let key = entry.path.to_ascii_lowercase();
        if key.is_empty() || !seen_files.insert(key) {
            continue;
        }
        dst.files.push(entry);
    }

    let mut seen_diffs = dst
        .diffs
        .iter()
        .map(|entry| {
            format!(
                "{}|{}",
                entry
                    .path
                    .as_deref()
                    .unwrap_or_default()
                    .to_ascii_lowercase(),
                entry.summary.to_ascii_lowercase()
            )
        })
        .collect::<HashSet<_>>();
    for entry in src.diffs {
        let key = format!(
            "{}|{}",
            entry
                .path
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase(),
            entry.summary.to_ascii_lowercase()
        );
        if key == "|" || !seen_diffs.insert(key) {
            continue;
        }
        dst.diffs.push(entry);
    }

    let mut seen_commands = dst
        .commands
        .iter()
        .map(|entry| entry.cmd.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    for entry in src.commands {
        let key = entry.cmd.to_ascii_lowercase();
        if key.is_empty() || !seen_commands.insert(key) {
            continue;
        }
        dst.commands.push(entry);
    }

    let mut seen_risks = dst
        .risks
        .iter()
        .map(|entry| entry.item.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    for entry in src.risks {
        let key = entry.item.to_ascii_lowercase();
        if key.is_empty() || !seen_risks.insert(key) {
            continue;
        }
        dst.risks.push(entry);
    }

    let mut seen_notes = dst
        .notes
        .iter()
        .map(|note| note.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    for note in src.notes {
        let key = note.to_ascii_lowercase();
        if key.is_empty() || !seen_notes.insert(key) {
            continue;
        }
        dst.notes.push(note);
    }
}

fn blocked_on(run: &SwarmRun, task: &SwarmTask) -> Vec<String> {
    task.deps
        .iter()
        .filter_map(|dep_id| {
            let dep = run.tasks.iter().find(|candidate| candidate.id == *dep_id)?;
            (!dep.state.is_terminal()).then(|| dep.id.clone())
        })
        .collect()
}

fn task_state_dashboard_label(state: SwarmTaskState) -> &'static str {
    match state {
        SwarmTaskState::Pending => "Pending",
        SwarmTaskState::Ready | SwarmTaskState::Dispatched => "Queued",
        SwarmTaskState::Running => "Running",
        SwarmTaskState::Done => "Done",
        SwarmTaskState::Failed => "Failed",
        SwarmTaskState::Skipped => "Skipped",
    }
}

fn stage_label(stage: SwarmStage) -> &'static str {
    match stage {
        SwarmStage::Planning => "PLAN",
        SwarmStage::Executing => "EXEC",
        SwarmStage::Verifying => "VERIFY",
        SwarmStage::Synthesizing => "SYNTH",
    }
}

fn dashboard_gate_rows(run: &SwarmRun) -> Vec<SwarmGateDashboardRow> {
    let mut rows = Vec::new();
    if let Some(bundle) = run.gate_bundle.as_ref() {
        for gate in bundle.gates() {
            rows.push(SwarmGateDashboardRow {
                name: gate.name.to_string(),
                command: gate.command.to_string(),
                status: "PENDING".into(),
                notes: None,
            });
        }
    }
    if let Some(report) = run.gate_report.as_ref() {
        for reported in report.gates.iter() {
            if let Some(existing) = rows.iter_mut().find(|row| row.name == reported.name) {
                existing.status = reported.ui_status().into();
                existing.command = reported.command.clone();
                existing.notes = reported.notes.clone();
            } else {
                rows.push(SwarmGateDashboardRow {
                    name: reported.name.clone(),
                    command: reported.command.clone(),
                    status: reported.ui_status().into(),
                    notes: reported.notes.clone(),
                });
            }
        }
    }
    rows
}

fn gate_bundle_label(bundle: Option<&GateBundle>, source: &str) -> String {
    let source = source.trim();
    if source.is_empty() {
        return bundle
            .map(|bundle| bundle.label().to_string())
            .unwrap_or_else(|| "(none)".into());
    }
    if source.eq_ignore_ascii_case("config:none") {
        return "none (config)".into();
    }
    match bundle {
        Some(bundle) => format!("{} ({source})", bundle.label()),
        None => format!("(none) ({source})"),
    }
}

fn read_workspace_gate_default(workspace_root: &Path) -> Result<Option<String>, String> {
    let path = workspace_root.join(".nit").join("config.toml");
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path)
        .map_err(|err| format!("failed reading {}: {err}", path.display()))?;
    let value = toml::from_str::<toml::Value>(&contents)
        .map_err(|err| format!("failed parsing {}: {err}", path.display()))?;
    Ok(value
        .get("swarm")
        .and_then(|value| value.get("gates"))
        .and_then(|value| value.get("default"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase()))
}

fn read_workspace_dag_validation_mode(
    workspace_root: &Path,
) -> Result<Option<SwarmDagValidationMode>, String> {
    let path = workspace_root.join(".nit").join("config.toml");
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path)
        .map_err(|err| format!("failed reading {}: {err}", path.display()))?;
    let value = toml::from_str::<toml::Value>(&contents)
        .map_err(|err| format!("failed parsing {}: {err}", path.display()))?;
    let Some(mode) = value
        .get("swarm")
        .and_then(|value| value.get("dag_validation"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let mode = mode.to_ascii_lowercase();
    if mode == "strict" || mode == "hard-fail" || mode == "hard_fail" || mode == "hardfail" {
        return Ok(Some(SwarmDagValidationMode::Strict));
    }
    if mode == "repair"
        || mode == "best-effort"
        || mode == "best_effort"
        || mode == "auto-repair"
        || mode == "auto_repair"
    {
        return Ok(Some(SwarmDagValidationMode::Repair));
    }

    Err(format!(
        "invalid swarm.dag_validation value '{mode}' (expected 'strict' or 'repair')"
    ))
}

fn sanitize_for_filename(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Extract directory/module paths from the operator prompt and enumerate their
/// source files.  Returns relative paths sorted alphabetically, capped at 100
/// entries to keep the planner prompt sane.
pub(crate) fn enumerate_scope_files(workspace_root: &Path, prompt: &str) -> Vec<String> {
    // Look for path-like tokens that point to directories inside the workspace.
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    for token in prompt.split_whitespace() {
        let token = token.trim_matches(|c: char| c == ',' || c == '.' || c == '"' || c == '\'');
        if token.is_empty() {
            continue;
        }
        // Must look like a path (contains / or starts with "crates/", "src/", etc.)
        if !token.contains('/') {
            continue;
        }
        let candidate = workspace_root.join(token);
        if candidate.is_dir() {
            dirs.push(candidate);
        }
    }
    if dirs.is_empty() {
        return Vec::new();
    }

    let mut files = Vec::new();
    for dir in dirs.iter() {
        collect_source_files(dir, workspace_root, &mut files);
    }
    files.sort();
    files.dedup();
    files.truncate(100);
    files
}

fn collect_source_files(dir: &Path, workspace_root: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip hidden dirs and target/
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') || name == "target" {
                    continue;
                }
            }
            collect_source_files(&path, workspace_root, out);
        } else if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(
                    ext,
                    "rs" | "toml" | "ts" | "js" | "py" | "go" | "c" | "h" | "cpp" | "hpp"
                ) {
                    if let Ok(rel) = path.strip_prefix(workspace_root) {
                        out.push(rel.display().to_string());
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_planner_prompt(
    root_prompt: &str,
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    planner_agent_id: &str,
    agent_ids: &[String],
    integrator_agent_id: Option<&str>,
    role_hints: &[(String, String)],
    priority_agent_ids: &[String],
    workspace_root: &Path,
) -> String {
    let available = agent_ids
        .iter()
        .filter(|id| id.as_str() != planner_agent_id)
        .cloned()
        .collect::<Vec<_>>();
    let scope_files = enumerate_scope_files(workspace_root, root_prompt);
    let large_scope = scope_files.len() > 15;
    let mut out = String::new();
    out.push_str(
        "You are the SWARM PLANNER inside nit. Create an execution plan for a multi-agent workflow.\n\n",
    );
    out.push_str(&format!("Template: `{}`\n\n", template.label()));
    out.push_str(&format!("Mission focus: `{}`\n\n", mission_kind.label()));
    if let Some(integrator_agent_id) = integrator_agent_id {
        out.push_str(&format!(
            "Single-writer integrator: `{integrator_agent_id}` (only this agent may do workspace writes, and only this agent may receive the `integrate` role).\n\n"
        ));
    } else if matches!(template, SwarmTemplate::Lab | SwarmTemplate::Bulk) {
        out.push_str("Single-writer integrator: (none)\n\n");
    }

    out.push_str("Constraints:\n");
    out.push_str("- Only assign tasks to these agent ids:\n");
    for id in available.iter() {
        out.push_str(&format!("  - {id}\n"));
    }
    if matches!(template, SwarmTemplate::Parallel | SwarmTemplate::Bulk) && !role_hints.is_empty() {
        out.push_str("- Agent role hints (from roster; 'all' means no constraint):\n");
        for (id, role) in role_hints.iter() {
            out.push_str(&format!("  - {id}: {role}\n"));
        }
        out.push_str(
            "- Prefer assigning tasks whose `role` matches each agent's hint (unless role=all).\n",
        );
    }
    out.push_str(
        "- Role guide: use `research` for web/paper/resource exploration and idea discovery; use `computational-research` for tool-assisted or quantitative research, experiments, and evidence gathering.\n",
    );
    out.push_str(
        "- Reserve `research`/`computational-research` for topic investigation and strategy discovery, not routine codebase recon, unless the operator explicitly wants outside research.\n",
    );
    out.push_str(
        "- `computational-research` is the broad computation-heavy lane: simulations, modeling, numerical methods, optimization, data/model fitting, pattern or network analysis, reproducibility, and research-computing workflows across technical domains.\n",
    );
    out.push_str(
        "- If you assign `research` or `computational-research`, ensure the task output asks for sources, methods, assumptions, and ranked strategy recommendations.\n",
    );
    match mission_kind {
        SwarmMissionKind::General => out.push_str(
            "- This mission is not research-oriented, so avoid `research` / `computational-research` roles unless the operator explicitly changes the mission focus.\n",
        ),
        SwarmMissionKind::Research => {
            out.push_str(
                "- This is a research mission: prefer a workflow like source survey -> evidence comparison -> synthesis / ranked strategy recommendation.\n",
            );
            out.push_str(
                "- `research` is the primary mission-specific role here; only use `computational-research` if the mission clearly needs simulations, modeling, or quantitative analysis.\n",
            );
            out.push_str(
                "- Prefer read-only investigation and synthesis tasks unless the operator explicitly asked for repo edits or docs changes.\n",
            );
        }
        SwarmMissionKind::ComputationalResearch => {
            out.push_str(
                "- This is a computational-research mission: prefer a workflow like source survey -> modeling / experiments / analysis -> synthesis / ranked strategy recommendation.\n",
            );
            out.push_str(
                "- `computational-research` is valid and preferred for quantitative or tool-driven lanes; `research` can support source survey and literature/context gathering.\n",
            );
            out.push_str(
                "- Prefer read-only investigation and synthesis tasks unless the operator explicitly asked for repo edits or docs changes.\n",
            );
        }
    }
    if matches!(template, SwarmTemplate::Parallel | SwarmTemplate::Bulk) {
        if large_scope {
            out.push_str(
                "- Treat `judge` as a singleton role. The `integrate` role may be split across MULTIPLE tasks when the scope is large — assign disjoint file subsets to each integrate task so every file is covered.\n",
            );
        } else {
            out.push_str(
                "- Treat `judge` and `integrate` as singleton roles: assign at most one task for each role unless the operator explicitly asks for duplicates.\n",
            );
        }
    }
    if let Some(integrator_agent_id) = integrator_agent_id {
        if large_scope {
            out.push_str(&format!(
                "- Code changes: assign `writes=true` and `role=integrate` to tasks. The scope has {} files — split integrate work across multiple agents with disjoint file subsets. Each integrate task prompt MUST list the exact files it is responsible for. Any agent may receive `role=integrate` and `writes=true` when the scope is large.\n",
                scope_files.len()
            ));
        } else {
            out.push_str(&format!(
                "- If code changes are needed, assign `writes=true` and `role=integrate` only to `{integrator_agent_id}`.\n"
            ));
        }
        if matches!(mission_kind, SwarmMissionKind::General) {
            if large_scope {
                out.push_str(
                    "- REQUIRED: for code-change, refactoring, or implementation requests you MUST include at least one task with `role=integrate` and `writes=true`. Split across multiple integrate tasks so every file in the scope is covered. Without integrate tasks, no workspace edits will be made.\n"
                );
            } else {
                out.push_str(&format!(
                    "- REQUIRED: for code-change, refactoring, or implementation requests you MUST include exactly one task with `role=integrate` and `writes=true` assigned to `{integrator_agent_id}`. Without an integrate task, no workspace edits will be made and the swarm will produce no changes.\n"
                ));
            }
        }
    }
    if matches!(template, SwarmTemplate::Parallel | SwarmTemplate::Bulk)
        && !priority_agent_ids.is_empty()
    {
        out.push_str("- Priority agents (from roster):\n");
        for id in priority_agent_ids.iter() {
            out.push_str(&format!("  - {id}\n"));
        }
        out.push_str(
            "- When multiple assignments are viable, prefer priority agents for the most critical/high-impact work.\n",
        );
    }
    match template {
        SwarmTemplate::Parallel => {
            out.push_str(
                "- Prefer ONE task per agent id (max parallelism, deterministic tracking).\n",
            );
            out.push_str(
                "- Prefer tasks that can run in parallel (deps should usually be empty).\n",
            );
            out.push_str(
                "- If you assign producer/consumer-style roles (e.g. research or computational-research → judge), use deps to express required ordering.\n",
            );
            out.push_str(
                "- Use `propose`, `research`, `review`, and `test` for the remaining lanes instead of repeating singleton roles.\n",
            );
        }
        SwarmTemplate::Lab => {
            out.push_str(
                "- You MAY assign multiple tasks to the same agent id (they run sequentially).\n",
            );
            out.push_str("- Use deps to express ordering (DAG). Avoid cycles.\n");
            out.push_str("- Only the integrator agent may have `writes=true` tasks.\n");
            out.push_str(
                "- Use read-only proposal/review tasks for codebase work; use research roles only when external/topic research is part of the mission.\n",
            );
        }
        SwarmTemplate::Bulk => {
            out.push_str(
                "- Bulk orchestration: explore multiple solution candidates in parallel, then converge.\n",
            );
            out.push_str(
                "- Prefer ONE proposer task per agent id (except the integrator), each with a distinct lens.\n",
            );
            out.push_str(
                "- Use ids `propose-01`, `propose-02`, ... plus `judge` and `integrate` so the workflow is easy to track.\n",
            );
            out.push_str(
                "- Create a judge task that depends on ALL proposer tasks and selects the best approach.\n",
            );
            out.push_str(
                "- Create an integrator task assigned to the integrator agent with `writes=true`, depending on the judge.\n",
            );
            out.push_str("- Use deps to express ordering (DAG). Avoid cycles.\n");
            out.push_str("- Only the integrator agent may have `writes=true` tasks.\n");
        }
    }
    out.push_str(
        "- When the operator request involves refactoring or modifying a module/directory, the plan MUST cover ALL files in that scope. Assign a recon or propose task to survey the full directory tree first, and ensure the integrate task prompt lists every affected file.\n",
    );
    out.push_str(
        "- Each task prompt should be specific about which files or areas to focus on, not generic. The more concrete the prompt, the better the agent output.\n",
    );
    out.push_str("\nOutput format:\n");
    out.push_str("1) 3-6 bullets summarizing the plan.\n");
    out.push_str("2) A JSON plan in a ```json code block with this schema (v2):\n");
    out.push_str("{\n");
    out.push_str("  \"version\": 2,\n");
    out.push_str(&format!("  \"template\": \"{}\",\n", template.label()));
    out.push_str("  \"integrator_agent_id\": \"(optional)\",\n");
    out.push_str("  \"tasks\": [\n");
    out.push_str("    {\n");
    out.push_str("      \"id\": \"task-id\",\n");
    out.push_str("      \"agent_id\": \"one-of-the-listed-agent-ids\",\n");
    out.push_str("      \"role\": \"(optional: propose|judge|research|computational-research|integrate|review|test)\",\n");
    out.push_str("      \"title\": \"short title\",\n");
    out.push_str("      \"prompt\": \"task instructions\",\n");
    out.push_str("      \"deps\": [\"task-id\"],\n");
    out.push_str("      \"writes\": false,\n");
    out.push_str(
        "      \"artifacts\": [\"(optional keys: files, diffs, commands, risks, notes)\"],\n",
    );
    out.push_str("      \"done_when\": \"(optional completion contract)\"\n");
    out.push_str("    }\n");
    out.push_str("  ],\n");
    out.push_str(
        "  \"synthesis_prompt\": \"(optional extra guidance for the final synthesis step)\"\n",
    );
    out.push_str("}\n");

    if !scope_files.is_empty() {
        out.push_str("\nScope — files in the referenced module/directory (");
        out.push_str(&format!("{} files):\n", scope_files.len()));
        for path in scope_files.iter() {
            out.push_str(&format!("  - {path}\n"));
        }
        out.push_str("\nSCOPE RULES:\n");
        out.push_str("- \"Refactor module\" means refactor EVERY file listed above. No file may remain unchanged.\n");
        out.push_str("- Each integrate task prompt MUST embed the exact file paths it is responsible for as a numbered checklist, e.g.:\n");
        out.push_str("  \"Refactor the following files. Open each file, read it, and apply improvements. Check off each file as you go:\\n1. crates/foo/src/bar.rs\\n2. crates/foo/src/baz.rs\\n...\"\n");
        out.push_str("- Distribute ALL files across integrate tasks so every file is assigned to exactly one task.\n");
        out.push_str("- If there is one integrate task, it must list all files. If there are multiple, split them into disjoint subsets.\n");
    }

    out.push_str("\nOperator request:\n");
    out.push_str(root_prompt.trim());
    out.push('\n');
    out
}

fn role_contract_lines(role: &str) -> &'static [&'static str] {
    match role {
        "propose" => &[
            "Advance one concrete solution candidate from your assigned lens.",
            "Do not judge between candidates or claim final implementation ownership.",
            "Be specific about files, commands, and risks.",
            "GENOME: nit measures the integrator's code across four encoders: token_spectrum (token role balance), ast_structure (tree shape variety, >= 5 components), complexity_field (cyclomatic complexity <= 8, identifier uniqueness >= 65%), structural (token-role diversity, AST depth variation, role n-gram uniqueness). See the full ENCODER GUIDE and TARGETS in the genome instructions attached to this prompt. Help the integrator score well — suggest function decomposition, varied patterns, and low-complexity approaches that target these encoders.",
        ],
        "research" => &[
            "Explore the topic through papers, docs, web resources, and related references when available.",
            "Surface competing ideas, promising directions, and the best strategy candidates with evidence.",
            "Do not turn this into a final implementation or winner-picking step; hand off concrete findings.",
        ],
        COMPUTATIONAL_RESEARCH_ROLE => &[
            "Handle the broad computation-heavy lane: simulations, modeling, numerical methods, optimization, data/model fitting, pattern or network analysis, and reproducible research workflows.",
            "Perform tool-assisted research with explicit methods, commands, sources, assumptions, and computations.",
            "Use the findings to recommend strong strategies or narrow the search space for downstream roles across technical domains.",
        ],
        "judge" => &[
            "Compare the dependency outputs and choose the best path forward.",
            "Produce a decisive recommendation, acceptance criteria, and verification steps.",
            "Do not edit the workspace or perform the final implementation.",
            "GENOME: nit measures code across four encoders: token_spectrum, ast_structure, complexity_field, structural. See the full ENCODER GUIDE and TARGETS in the genome instructions attached to this prompt. Prefer proposals that enable varied AST node types, low per-function complexity (<= 8), diverse token-role sequences, and >= 5 structural components. Flag proposals that would force monolithic functions or repetitive patterns.",
        ],
        "integrate" => &[
            "Implement the chosen plan and convert it into concrete edits.",
            "Do not restart broad ideation; focus on carrying the selected approach through.",
            "If a FILE CHECKLIST is provided above, you MUST modify every listed file — process them in order, one by one. A file left unchanged means your task is incomplete.",
            "Report exact files changed and validation results.",
            "CODE CONVENTION: Do NOT add inline test modules (`#[cfg(test)] mod tests { ... }`) inside source files. Tests must live in a dedicated tests directory or test file, not inline. If you encounter an existing inline test module during a refactor, move it to the appropriate test file/directory. Do NOT pad small files (lib.rs, mod.rs, re-export files) with unnecessary code to boost genome scores — trivially small files are auto-passed by the genome system. COMMENTS: Trim doc comments that restate the type/function name, echo visible type signatures, or describe obvious behavior. Keep comments that explain WHY, document non-obvious constraints, safety invariants, or algorithmic choices.",
            "GENOME QUALITY OBLIGATION: You are the sole writer. Your code is measured by nit's genome system across four encoders. See the full ENCODER GUIDE and TARGETS in the genome instructions attached to this prompt. Maintain or improve genome scores on every file you touch. Aim for Tier III+ (Spaceship) minimum, aspire to Tier V (Replicator). Do NOT call [evaluate_genome] — nit evaluates automatically after your changes are written to disk.",
        ],
        "review" => &[
            "Critique the current output or diff for correctness, UX, and maintainability.",
            "Call out risks, regressions, and missing tests.",
            "Do not edit the workspace; suggest follow-ups as text only.",
            "GENOME: nit measures code across four encoders. See the full ENCODER GUIDE and TARGETS in the genome instructions attached to this prompt. Name the affected encoder when flagging issues — e.g., 'complexity 12 in parse_config (complexity_field: target <= 8)', 'only 2 node types (ast_structure: need >= 5 components)', 'repeated role sequence across match arms (structural: role n-gram uniqueness)', 'comment-to-code ratio too low (token_spectrum)'. Suggest concrete refactoring the integrator can apply.",
        ],
        "test" => &[
            "Focus on validation commands, expected results, and edge cases.",
            "Differentiate confirmed results from unrun suggestions.",
            "Do not redesign the solution unless a test failure makes it necessary.",
        ],
        "genome-reviewer" => &[
            "Evaluate the structural quality of code changes using the genome reports provided.",
            "For each modified file, compare before/after genome metrics and identify regressions.",
            "Produce a structured review: which files improved, which regressed, critical issues, and specific refactoring recommendations.",
            "Overall verdict: PASS (all files tier III+ Spaceship) or FAIL (any file below tier III). Aspiration is tier V (Replicator).",
            "Do not edit the workspace; report findings as text only.",
        ],
        _ => &[
            "Stay within the assigned task scope.",
            "Do not silently switch into a different swarm role.",
        ],
    }
}

fn role_response_format_lines(role: &str) -> Option<&'static [&'static str]> {
    match role {
        "research" | COMPUTATIONAL_RESEARCH_ROLE => Some(&[
            "Sources: list the key papers, docs, web resources, or datasets you relied on.",
            "Methods: explain how you searched, compared, computed, simulated, or evaluated the topic.",
            "Assumptions: call out the main assumptions, uncertainties, and missing information.",
            "Ranked strategies: provide the best options in ranked order with brief rationale and tradeoffs.",
        ]),
        _ => None,
    }
}

fn wrap_task_prompt(
    root_prompt: &str,
    mission_kind: SwarmMissionKind,
    task: &SwarmTask,
    deps: Option<&[(String, String)]>,
    scope_files: &[String],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "SWARM TASK: {} ({})\n",
        task.title.trim(),
        task.id
    ));
    if let Some(role) = task.role.as_deref() {
        if !role.trim().is_empty() {
            out.push_str(&format!("ROLE: {}\n", role.trim()));
        }
    }
    if task.writes {
        out.push_str("MODE: single-writer integrator (workspace writes allowed)\n");
    } else {
        out.push_str("MODE: read-only (do not edit the workspace)\n");
    }
    if mission_kind.allows_research_roles() {
        out.push_str(&format!("MISSION FOCUS: {}\n", mission_kind.label()));
        out.push_str("MISSION CONTRACT:\n");
        match mission_kind {
            SwarmMissionKind::Research => out.push_str(
                "- This is a research mission: prioritize external sources, evidence, and ranked strategy discovery over routine code implementation.\n",
            ),
            SwarmMissionKind::ComputationalResearch => out.push_str(
                "- This is a computational-research mission: prioritize modeling, experiments, quantitative evidence, and reproducible analysis over routine code implementation.\n",
            ),
            SwarmMissionKind::General => {}
        }
    }
    if let Some(role) = task.role.as_deref().and_then(normalize_role_label) {
        out.push_str("ROLE CONTRACT:\n");
        out.push_str("- Act strictly as the assigned role for this task.\n");
        for line in role_contract_lines(role.as_str()) {
            out.push_str(&format!("- {line}\n"));
        }
        if let Some(lines) = role_response_format_lines(role.as_str()) {
            out.push_str("RESPONSE FORMAT:\n");
            for line in lines {
                out.push_str(&format!("- {line}\n"));
            }
        }
    }
    if let Some(done_when) = task.done_when.as_deref() {
        if !done_when.trim().is_empty() {
            out.push_str(&format!("DONE WHEN: {}\n", done_when.trim()));
        }
    }
    if !task.artifacts.is_empty() {
        out.push_str("ARTIFACTS:\n");
        for item in task.artifacts.iter() {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            out.push_str(&format!("- {item}\n"));
        }
    }

    out.push_str("\nOperator request:\n");
    out.push_str(root_prompt.trim());
    out.push('\n');

    // Inject the scope file list BEFORE the task prompt so the agent sees the
    // full file checklist first, then the task instructions.  This prevents the
    // agent from forming a plan that ignores files.
    if !scope_files.is_empty() {
        let is_integrate = task
            .role
            .as_deref()
            .and_then(normalize_role_label)
            .as_deref()
            == Some("integrate");
        let is_propose = task
            .role
            .as_deref()
            .and_then(normalize_role_label)
            .as_deref()
            == Some("propose");
        if is_integrate {
            out.push_str("\n## FILE CHECKLIST (non-negotiable)\n");
            out.push_str(
                "\"Refactor module\" = refactor EVERY file below. No exceptions, no skipping.\n",
            );
            out.push_str("Process this checklist in order. Open each file, read it, refactor it, then move to the next.\n");
            out.push_str(
                "Even if a file looks clean, improve naming, docs, structure, or consistency.\n",
            );
            out.push_str("Do NOT add inline test modules (`#[cfg(test)] mod tests { ... }`) inside source files. Tests must live in a dedicated tests directory or test file.\n");
            out.push_str("COMMENTS: Trim doc comments that restate the type/function name, echo visible type signatures, or describe obvious behavior (e.g. \"/// Returns the value\" on fn value()). Keep comments that explain WHY something is done, document non-obvious constraints, safety invariants, or algorithmic choices. A comment worth keeping tells the reader something the code alone cannot.\n");
            out.push_str("Your task is NOT complete until every file has been modified.\n\n");
            for (i, path) in scope_files.iter().enumerate() {
                out.push_str(&format!("{}. {path}\n", i + 1));
            }
            out.push_str("\nAfter finishing, list every file and what you changed in each.\n");
        } else if is_propose {
            out.push_str("\n## SCOPE — files in the target module\n");
            out.push_str("Your proposal must cover ALL of these files (no exceptions):\n");
            for (i, path) in scope_files.iter().enumerate() {
                out.push_str(&format!("{}. {path}\n", i + 1));
            }
        }
    }

    out.push_str("\nYour task:\n");
    out.push_str(task.task_prompt.trim());
    out.push('\n');

    if let Some(deps) = deps {
        if !deps.is_empty() {
            let is_judge = task
                .role
                .as_deref()
                .and_then(normalize_role_label)
                .as_deref()
                == Some("judge");
            if is_judge {
                out.push_str(&format!(
                    "\nDependency outputs ({} proposals to evaluate — read ALL of them carefully before choosing):\n",
                    deps.len()
                ));
            } else {
                out.push_str("\nDependency outputs:\n");
            }
            for (label, output) in deps.iter() {
                out.push_str(&format!("\n---\nDEP: {label}\n"));
                out.push_str(output.trim());
                out.push('\n');
            }
        }
    }

    if !task.artifacts.is_empty() {
        out.push_str("\n## STRUCTURED ARTIFACTS (REQUIRED)\n");
        out.push_str("You MUST include a ```json code block at the END of your response with this exact structure:\n");
        out.push_str("```\n");
        out.push_str("{\n");
        out.push_str("  \"type\": \"swarm_artifacts\",\n");
        out.push_str("  \"version\": 1,\n");
        out.push_str(&format!("  \"task_id\": \"{}\",\n", task.id));
        out.push_str("  \"summary\": \"one-line summary of what you did or found\",\n");
        out.push_str("  \"artifacts\": {\n");
        out.push_str("    \"files\": [\"path/to/file.rs\"],\n");
        out.push_str("    \"diffs\": [{\"path\": \"file.rs\", \"summary\": \"what changed\"}],\n");
        out.push_str("    \"commands\": [\"cargo test\"],\n");
        out.push_str("    \"risks\": [\"potential issue\"],\n");
        out.push_str("    \"notes\": [\"additional context\"]\n");
        out.push_str("  }\n");
        out.push_str("}\n");
        out.push_str("```\n");
        out.push_str("Only include artifact keys relevant to your task. This JSON block is machine-parsed by the swarm orchestrator — omitting it means your output cannot be tracked.\n");
    }

    out.push_str("\nRespond with:\n- Findings / recommendations\n- Concrete file paths / commands where relevant\n");

    // Embed genome quality instructions so every role is aware of the measurement system,
    // regardless of whether genome context is also injected at dispatch time.
    out.push('\n');
    out.push_str(nit_core::GENOME_AGENT_INSTRUCTIONS);
    out.push('\n');

    out
}

fn build_synthesis_prompt(run: &SwarmRun) -> String {
    let mut out = String::new();
    out.push_str(
        "You are the SWARM SYNTHESIZER. Produce a single cohesive response for the operator by combining the parallel agent outputs below.\n\n",
    );
    out.push_str("Operator request:\n");
    out.push_str(run.root_prompt.trim());
    out.push_str("\n\nAgent outputs:\n");
    for task in run.tasks.iter() {
        out.push_str(&format!(
            "\n---\nAGENT: {}\nTASK: {} ({})\n",
            task.agent_id,
            task.title.trim(),
            task.id
        ));
        if let Some(role) = task.role.as_deref() {
            if !role.trim().is_empty() {
                out.push_str(&format!("ROLE: {}\n", role.trim()));
            }
        }
        if !task.deps.is_empty() {
            out.push_str(&format!("DEPS: {}\n", task.deps.join(", ")));
        }
        let status = match task.state {
            SwarmTaskState::Done => "DONE",
            SwarmTaskState::Failed => "FAILED",
            SwarmTaskState::Skipped => "SKIPPED",
            SwarmTaskState::Pending => "PENDING",
            SwarmTaskState::Ready => "READY",
            SwarmTaskState::Dispatched => "QUEUED",
            SwarmTaskState::Running => "RUNNING",
        };
        out.push_str(&format!("STATUS: {status}\n"));
        if let Some(summary) = task_artifacts_summary_for_prompt(task, &run.mission_id) {
            out.push_str("ARTIFACTS:\n");
            out.push_str(summary.trim());
            out.push('\n');
        } else if task.expected_artifacts_missing {
            out.push_str("ARTIFACTS: expected but missing parseable swarm_artifacts JSON block\n");
        }
        if let Some(output) = task.output.as_deref() {
            out.push_str(output.trim());
            out.push('\n');
        } else {
            out.push_str("(no output)\n");
        }
    }
    if let Some(bundle) = run.gate_bundle.as_ref() {
        out.push_str("\n\nVerification gates:\n");
        out.push_str(&format!("Bundle: {}\n", bundle.label()));
        for gate in dashboard_gate_rows(run).iter() {
            out.push_str(&format!(
                "- {}: {} ({})\n",
                gate.name, gate.status, gate.command
            ));
        }
        if let Some(report) = run.gate_report.as_ref() {
            out.push_str("Structured report:\n```json\n");
            if let Ok(json) = serde_json::to_string_pretty(report) {
                out.push_str(&json);
            } else {
                out.push_str("{\"overall_ok\":false}");
            }
            out.push_str("\n```\n");
        } else {
            out.push_str("Structured report: (missing)\n");
        }
        if let Some(output) = run.gate_output.as_deref() {
            out.push_str("\nVerifier raw output (truncated):\n");
            out.push_str(&truncate_chars(output, SWARM_VERIFY_MAX_CHARS));
            out.push('\n');
        }
    }
    if let Some(genome_results) = run.genome_gate_results.as_deref() {
        out.push_str("\n\nGenome quality review:\n");
        out.push_str(genome_results);
        out.push('\n');
    }
    if let Some(extra) = run.synthesis_prompt.as_deref() {
        out.push_str("\n\nSynthesis notes:\n");
        out.push_str(extra.trim());
        out.push('\n');
    }
    out.push_str(
        "\nResponse requirements:\n- Be decisive: choose a best approach.\n- Include specific next steps.\n- If code changes are needed, outline exact edits and validation steps.\n",
    );
    out
}

fn extract_json_code_block(text: &str) -> Option<String> {
    if let Some(first) = extract_json_code_blocks(text).into_iter().next() {
        return Some(first);
    }

    // Fallback: attempt to parse the first JSON object we can find.
    let trimmed = text.trim();
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if start >= end {
        return None;
    }
    let candidate = trimmed[start..=end].trim().to_string();
    (!candidate.is_empty()).then_some(candidate)
}

fn extract_json_code_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let is_json_fence = trimmed.starts_with("```json") || trimmed.starts_with("```JSON");
        if !is_json_fence {
            continue;
        }
        let mut buf = String::new();
        for inner in &mut lines {
            if inner.trim() == "```" {
                break;
            }
            buf.push_str(inner);
            buf.push('\n');
        }
        let candidate = buf.trim().trim_end_matches('`').trim().to_string();
        if !candidate.is_empty() {
            blocks.push(candidate);
        }
    }
    blocks
}

/// Build the genome review prompt for the genome-reviewer role.
fn build_genome_review_prompt(state: &AppState) -> String {
    let mut prompt = String::from(
        "You are the genome reviewer in nit's coding lab. nit measures structural code \
         quality by encoding source files as Game of Life genomes. The lab's goal is to \
         produce elite Replicator-tier (Tier V, 2001+ generations) code. Evaluate the \
         structural quality of the code changes made by this swarm mission. For each \
         modified file, a genome report shows before/after metrics across four encoders.\n\n",
    );

    // Evaluate ALL modified files, not just the editor buffer.
    let mut files_to_eval: Vec<std::path::PathBuf> = state
        .genome_turn_modified
        .values()
        .flat_map(|s| s.iter().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    if let Some(editor_path) = state.editor_buffer().path().cloned() {
        if !files_to_eval.contains(&editor_path) {
            files_to_eval.push(editor_path);
        }
    }
    files_to_eval.sort();

    let mut has_content = false;
    for file_path in &files_to_eval {
        let text = match std::fs::read_to_string(file_path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let report = nit_core::compute_genome_report(&text, file_path);
        prompt.push_str(&format!("--- {} ---\n", file_path.display()));
        prompt.push_str(&nit_core::format_genome_report(&report));
        prompt.push('\n');

        if let Some(prev) = state.genome_reports.get(file_path) {
            let diff = nit_core::compute_genome_diff(prev, &report);
            prompt.push_str(&nit_core::format_genome_diff(&diff));
            prompt.push('\n');
        }
        has_content = true;
    }

    if !has_content {
        return String::new();
    }

    prompt.push_str(
        "\nProduce a structured review:\n\
         1. Which files improved in structural quality and which regressed\n\
         2. The most critical structural issues remaining\n\
         3. Specific refactoring recommendations for the worst-scoring files\n\
         4. Overall verdict: PASS (all files tier III+ Spaceship) or FAIL (any file below tier III)\n\
         5. Distance from Replicator (Tier V) — what would it take to reach elite status\n",
    );

    prompt
}

/// Evaluate genome quality on ALL modified files and produce a gate result string.
fn evaluate_genome_gate(state: &AppState) -> String {
    let genome_config = &state.settings.genome.genome_gate;
    let min_tier = nit_core::GenomeTier::from_generations(match genome_config.min_tier {
        0 => 0,
        1 => 51,
        2 => 201,
        3 => 501,
        _ => 2001,
    });

    // Collect all files to evaluate: modified files + editor buffer.
    let mut files_to_eval: Vec<std::path::PathBuf> = state
        .genome_turn_modified
        .values()
        .flat_map(|s| s.iter().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    if let Some(editor_path) = state.editor_buffer().path().cloned() {
        if !files_to_eval.contains(&editor_path) {
            files_to_eval.push(editor_path);
        }
    }
    files_to_eval.sort();

    let mut out = String::new();
    let mut all_failures: Vec<String> = Vec::new();
    let mut file_count = 0u32;
    let mut pass_count = 0u32;

    for file_path in &files_to_eval {
        let text = match std::fs::read_to_string(file_path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let report = nit_core::compute_genome_report(&text, file_path);
        let mut failures = Vec::new();
        file_count += 1;

        if report.tier < min_tier {
            failures.push(format!(
                "Genome FAIL: {} tier {} ({}) below minimum {} ({})",
                file_path.display(),
                report.tier.numeral(),
                report.tier.name(),
                min_tier.numeral(),
                min_tier.name(),
            ));
        }

        for score in &report.encoder_scores {
            if matches!(
                score.encoder,
                nit_core::SeedEncoderId::TokenSpectrum
                    | nit_core::SeedEncoderId::AstStructure
                    | nit_core::SeedEncoderId::ComplexityField
            ) && score.density > genome_config.max_density
            {
                failures.push(format!(
                    "Genome FAIL: {} density {:.2} on {} exceeds {:.2}",
                    file_path.display(),
                    score.density,
                    score.encoder.label(),
                    genome_config.max_density,
                ));
            }
        }

        if let Some(s) = report
            .encoder_scores
            .iter()
            .find(|s| s.encoder == nit_core::SeedEncoderId::AstStructure)
        {
            if s.components < genome_config.min_components {
                failures.push(format!(
                    "Genome FAIL: {} has {} components (min: {})",
                    file_path.display(),
                    s.components,
                    genome_config.min_components,
                ));
            }
        }

        if report.cross_encoder_consistency < genome_config.min_consistency {
            failures.push(format!(
                "Genome FAIL: {} consistency {:.2} below {:.2}",
                file_path.display(),
                report.cross_encoder_consistency,
                genome_config.min_consistency,
            ));
        }

        if report.parsimony.bloat_detected {
            failures.push(format!(
                "Genome FAIL: {} parsimony bloat detected (tier capped at IV). \
                 {} fns avg {:.1} lines, {:.0}% tiny, {:.0}% comments. \
                 Consolidate over-split functions, remove comment padding, \
                 inline trivial predicates.",
                file_path.display(),
                report.parsimony.fn_count,
                report.parsimony.avg_fn_body_lines,
                report.parsimony.tiny_fn_fraction * 100.0,
                report.parsimony.comment_ratio * 100.0,
            ));
        }

        if genome_config.require_no_regression {
            if let Some(prev) = state.genome_reports.get(file_path) {
                if report.tier < prev.tier {
                    failures.push(format!(
                        "Genome FAIL: {} regressed from {} ({}) to {} ({})",
                        file_path.display(),
                        prev.tier.numeral(),
                        prev.tier.name(),
                        report.tier.numeral(),
                        report.tier.name(),
                    ));
                }
            }
        }

        for rec in &report.recommendations {
            if matches!(rec.severity, nit_core::RecommendationSeverity::Critical) {
                failures.push(format!("  Recommendation: {}", rec.message));
            }
        }

        out.push_str(&format!("--- {} ---\n", file_path.display()));
        out.push_str(&nit_core::format_genome_report(&report));
        if failures.is_empty() {
            out.push_str(&format!("  Result: PASS ({})\n\n", report.quality_level()));
            pass_count += 1;
        } else {
            out.push_str(&format!("  Result: FAIL ({})\n", report.quality_level()));
            for f in &failures {
                out.push_str(&format!("  {f}\n"));
            }
            out.push('\n');
            all_failures.extend(failures);
        }
    }

    // Summary.
    if file_count == 0 {
        out.push_str("Genome gate: SKIP (no files to evaluate)\n");
    } else if all_failures.is_empty() {
        out.push_str(&format!(
            "Genome gate: PASS ({pass_count}/{file_count} files passed)\n"
        ));
    } else {
        out.push_str(&format!(
            "Genome gate: FAIL ({pass_count}/{file_count} files passed, {} failures)\n",
            all_failures.len(),
        ));
    }
    out
}

fn build_verify_prompt(run: &SwarmRun, bundle: &GateBundle) -> String {
    let mut out = String::new();
    out.push_str(
        "You are the SWARM VERIFIER. Run the verification gate bundle below against the current workspace.\n\n",
    );
    out.push_str("Rules:\n");
    out.push_str("- Run commands in order.\n");
    out.push_str(
        "- If a gate fails, keep going when feasible (collect as much signal as possible).\n",
    );
    out.push_str("- Keep logs concise: include only the key error snippets needed to debug.\n");
    out.push_str("- At the end, output a single JSON report in a ```json code block.\n");
    out.push_str("\nOperator request (context):\n");
    out.push_str(run.root_prompt.trim());
    out.push_str("\n\nGate bundle:\n");
    out.push_str(&format!("Bundle: {}\n", bundle.label()));
    for gate in bundle.gates() {
        out.push_str(&format!("- {}: `{}`\n", gate.name, gate.command));
    }

    if let Some(genome_results) = run.genome_gate_results.as_deref() {
        out.push_str("\nGenome gate (pre-evaluated by nit):\n");
        out.push_str(genome_results);
        out.push_str("\nInclude a gate entry for \"genome-quality\" with ok=true/false based on the results above.\n");
    }

    out.push_str("\nReport schema:\n");
    out.push_str("{\"overall_ok\":true,\"gates\":[{\"name\":\"fmt\",\"command\":\"...\",\"ok\":true,\"status\":\"pass|fail|skip\",\"notes\":\"(optional)\"}]}\n");
    out.push_str(
        "\nImportant: The JSON must reflect the actual command outcomes (ok=true only when the command succeeded).\n",
    );
    out
}

fn parse_gate_report(message: &str) -> Option<GateReport> {
    for json in extract_json_code_blocks(message) {
        if let Ok(report) = serde_json::from_str::<GateReport>(&json) {
            return Some(report);
        }
    }
    let json = extract_json_code_block(message)?;
    serde_json::from_str::<GateReport>(&json).ok()
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let clipped: String = text.chars().take(max_chars).collect();
    format!("{clipped}\n... (truncated)")
}

fn next_mission_id(state: &AppState) -> String {
    format!("mis-{:03}", state.agents.missions.len() + 1)
}

fn swarm_mission_title(
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

fn timestamp_label(state: &AppState) -> String {
    format!("t+{}", state.metrics.frame_count)
}

fn update_mission_phase(state: &mut AppState, mission_id: &str, phase: MissionPhase) {
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

fn abort_swarm_plan_preflight(state: &mut AppState, run: &mut SwarmRun, parsed: ParsedSwarmPlan) {
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

fn update_mission_final(state: &mut AppState, mission_id: &str, status: &str) {
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

fn update_mission_status(state: &mut AppState, run: &SwarmRun, done_override: Option<usize>) {
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
    .clamp(1, MAX_SWARM_SIZE);
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

    // Only use explicitly-selected priority agents — never pick random
    // models from the roster.  Any remaining slots will be filled by
    // ensure_size_clones with clones of the planner.
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
pub fn resolve_base_agent_id(agent_id: &str) -> &str {
    chat_clone_base_id(agent_id)
        .or_else(|| swarm_clone_base_id(agent_id))
        .unwrap_or(agent_id)
}

/// Returns true if the base agent **or** any of its clones is busy.
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

fn is_priority_agent(state: &AppState, agent_id: &str) -> bool {
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

/// Tag the most recent message from `agent_id` for `mission_id` with the given kind.
fn tag_last_agent_message_kind(state: &mut AppState, agent_id: &str, mission_id: &str, kind: &str) {
    if let Some(msg) = state.agents.messages.iter_mut().rev().find(|msg| {
        msg.agent_id.as_deref() == Some(agent_id) && msg.mission_id.as_deref() == Some(mission_id)
    }) {
        msg.kind = Some(kind.to_string());
    }
}

#[cfg(test)]
#[path = "tests/swarm.rs"]
mod tests;
