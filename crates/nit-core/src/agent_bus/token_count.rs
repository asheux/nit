use std::time::Instant;

use crate::state::AppState;

use super::AgentTokenCount;

pub(super) fn handle_token_count_event(
    state: &mut AppState,
    agent_id: &str,
    mission_id: Option<&str>,
    token_count: &AgentTokenCount,
) {
    if let Some(turn) = state.agents.active_turns.get_mut(agent_id) {
        turn.last_output_at = Instant::now();
    }
    apply_codex_token_count(state, agent_id, mission_id, token_count);
}

pub(crate) fn apply_codex_token_count(
    state: &mut AppState,
    agent_id: &str,
    mission_id: Option<&str>,
    token_count: &AgentTokenCount,
) {
    let is_claude = state
        .agents
        .agents
        .iter()
        .find(|a| a.id == agent_id)
        .is_some_and(|a| a.is_claude());

    if is_claude {
        apply_token_count_claude(state, agent_id, mission_id, token_count);
    } else {
        apply_token_count_codex(state, agent_id, mission_id, token_count);
    }
}

fn apply_token_count_codex(
    state: &mut AppState,
    agent_id: &str,
    mission_id: Option<&str>,
    token_count: &AgentTokenCount,
) {
    if token_count.context_window > 0 {
        state
            .agents
            .codex_effective_context_window_tokens
            .insert(agent_id.to_string(), token_count.context_window);
    }
    let context_window = if token_count.context_window > 0 {
        Some(token_count.context_window)
    } else {
        state
            .agents
            .codex_effective_context_window_tokens
            .get(agent_id)
            .copied()
    };

    let used = context_window
        .map(|window| token_count.total_tokens.min(window.max(1)))
        .unwrap_or(token_count.total_tokens);
    if let Some(mission_id) = mission_id {
        state
            .agents
            .codex_mission_used_tokens
            .entry(mission_id.to_string())
            .or_default()
            .insert(agent_id.to_string(), used);
    } else {
        state
            .agents
            .codex_used_tokens
            .insert(agent_id.to_string(), used);
    }
    let Some(context_window) = context_window else {
        return;
    };
    if context_window == 0 {
        return;
    }

    let pct = remaining_pct(context_window, used);

    if let Some(mission_id) = mission_id {
        state
            .agents
            .codex_mission_context_remaining_pct
            .entry(mission_id.to_string())
            .or_default()
            .insert(agent_id.to_string(), pct);
    } else {
        state
            .agents
            .codex_context_remaining_pct
            .insert(agent_id.to_string(), pct);
    }
}

fn apply_token_count_claude(
    state: &mut AppState,
    agent_id: &str,
    mission_id: Option<&str>,
    token_count: &AgentTokenCount,
) {
    if token_count.context_window > 0 {
        state
            .agents
            .claude_effective_context_window_tokens
            .insert(agent_id.to_string(), token_count.context_window);
    }
    let context_window = if token_count.context_window > 0 {
        Some(token_count.context_window)
    } else {
        state
            .agents
            .claude_effective_context_window_tokens
            .get(agent_id)
            .copied()
    };

    let used = context_window
        .map(|window| token_count.total_tokens.min(window.max(1)))
        .unwrap_or(token_count.total_tokens);
    if let Some(mission_id) = mission_id {
        state
            .agents
            .claude_mission_used_tokens
            .entry(mission_id.to_string())
            .or_default()
            .insert(agent_id.to_string(), used);
    } else {
        state
            .agents
            .claude_used_tokens
            .insert(agent_id.to_string(), used);
    }
    let Some(context_window) = context_window else {
        return;
    };
    if context_window == 0 {
        return;
    }

    let pct = remaining_pct(context_window, used);

    if let Some(mission_id) = mission_id {
        state
            .agents
            .claude_mission_context_remaining_pct
            .entry(mission_id.to_string())
            .or_default()
            .insert(agent_id.to_string(), pct);
    } else {
        state
            .agents
            .claude_context_remaining_pct
            .insert(agent_id.to_string(), pct);
    }
}

// Rounded percentage of remaining context, with banker-style mid-point bias
// (`+denom/2`) to avoid systematic flooring at 99% vs 100%.
fn remaining_pct(context_window: u32, used: u32) -> u8 {
    let remaining = context_window.saturating_sub(used);
    let denom = context_window as u64;
    (((remaining as u64).saturating_mul(100)).saturating_add(denom / 2) / denom) as u8
}
