use crate::state::{
    AgentAlertSeverity, AgentChannel, AgentDiagnosticEvent, AgentMessage, AgentStatus, AppState,
    CONSOLE_SCROLL_BOTTOM,
};

use super::helpers::{backend_source_for_agent, estimate_codex_context_tokens, timestamp_label};
use super::token_count::apply_codex_token_count;
use super::upsert::{mark_ad_hoc_provenance_dirty, mark_mission_provenance_dirty};
use super::AgentTokenCount;

pub(super) fn handle_turn_completed(
    state: &mut AppState,
    agent_id: &str,
    mission_id: &Option<String>,
    thread_id: &Option<String>,
    token_count: &Option<AgentTokenCount>,
    message: &str,
) {
    let source = backend_source_for_agent(state, agent_id);
    state.agents.active_turns.remove(agent_id);
    if let Some(token_count) = token_count.as_ref() {
        apply_codex_token_count(state, agent_id, mission_id.as_deref(), token_count);
    }
    let at = timestamp_label(state);
    if let Some(agent) = state.agents.agents_get_mut(agent_id) {
        agent.queue_len = agent.queue_len.saturating_sub(1);
        agent.status = if agent.queue_len > 0 {
            AgentStatus::Waiting
        } else {
            AgentStatus::Idle
        };
        agent.heartbeat_age_secs = 0;
        agent.current_mission = mission_id.clone();
    }

    update_provenance_and_threads(
        state,
        agent_id,
        mission_id.as_deref(),
        thread_id.as_deref(),
        &at,
    );
    bump_mission_token_estimate(state, mission_id.as_deref(), message);
    push_completion_message(state, agent_id, mission_id, message, &at);

    state.agents.diag_events.push(AgentDiagnosticEvent {
        severity: AgentAlertSeverity::Info,
        source: source.into(),
        message: format!("[{agent_id}] turn completed"),
        at,
    });

    // Editor buffer reload is handled by the file watcher on the next frame —
    // no synchronous I/O here to avoid blocking. Genome evaluation is dispatched
    // to background threads by the TUI event loop (genome_worker) for the
    // same reason.
    state.genome_turn_active.remove(agent_id);

    emit_done_marker(state, agent_id, mission_id, thread_id, message);
    advance_substrate_phase(state);
    run_observers_and_arbiters(state);

    let _ = state.substrate.save(&state.workspace_root);
}

fn update_provenance_and_threads(
    state: &mut AppState,
    agent_id: &str,
    mission_id: Option<&str>,
    thread_id: Option<&str>,
    at: &str,
) {
    if let Some(mission_id) = mission_id {
        mark_mission_provenance_dirty(state, mission_id);
        if let Some(thread_id) = thread_id {
            state
                .agents
                .codex_mission_thread_ids
                .entry(mission_id.to_string())
                .or_default()
                .insert(agent_id.to_string(), thread_id.to_string());
        }
        if let Some(mission) = state
            .agents
            .missions
            .iter_mut()
            .find(|mission| mission.id == mission_id)
        {
            mission.status = "LIVE".into();
            mission.updated_at = at.to_string();
        }
        return;
    }
    if let Some(thread_id) = thread_id {
        state
            .agents
            .codex_thread_ids
            .insert(agent_id.to_string(), thread_id.to_string());
    }
    mark_ad_hoc_provenance_dirty(state, agent_id);
}

fn bump_mission_token_estimate(state: &mut AppState, mission_id: Option<&str>, message: &str) {
    let Some(mission_id) = mission_id else {
        return;
    };
    let delta = estimate_codex_context_tokens(message);
    let entry = state
        .agents
        .codex_estimated_tokens_used_by_mission
        .entry(mission_id.to_string())
        .or_insert(0);
    *entry = entry.saturating_add(delta);
}

fn push_completion_message(
    state: &mut AppState,
    agent_id: &str,
    mission_id: &Option<String>,
    message: &str,
    at: &str,
) {
    // Use the dispatch-time prompt index if available; check both Codex and
    // Claude prompt-index maps. Fall back to scanning recent messages for an
    // operator prompt that targets this mission.
    let parent_prompt_idx = state
        .agents
        .codex_turn_prompt_idx
        .remove(agent_id)
        .or_else(|| state.agents.claude_turn_prompt_idx.remove(agent_id))
        .or_else(|| {
            state
                .agents
                .messages
                .iter()
                .enumerate()
                .rev()
                .find(|(_, msg)| msg.agent_id.is_none() && msg.mission_id == *mission_id)
                .map(|(idx, _)| idx)
        });
    state.agents.messages.push(AgentMessage {
        at: at.to_string(),
        channel: AgentChannel::Agent,
        agent_id: Some(agent_id.to_string()),
        mission_id: mission_id.clone(),
        text: message.to_string(),
        prompt_msg_idx: parent_prompt_idx,
        kind: None,
    });
    state.agents.console_scroll = CONSOLE_SCROLL_BOTTOM;
}

fn emit_done_marker(
    state: &mut AppState,
    agent_id: &str,
    mission_id: &Option<String>,
    thread_id: &Option<String>,
    message: &str,
) {
    let current_gen = state.substrate.current_generation();
    let id = state.substrate.next_signal_id(agent_id);
    state.substrate.emit_signal(crate::substrate::Signal {
        id,
        kind: crate::substrate::SignalKind::DoneMarker,
        posted_by: agent_id.to_string(),
        posted_at_gen: current_gen,
        target: crate::substrate::SignalTarget::Agent {
            agent_id: agent_id.to_string(),
        },
        initial_strength: crate::substrate::SubstrateState::DEFAULT_INITIAL_STRENGTH,
        payload: serde_json::json!({
            "message": message,
            "thread_id": thread_id,
            "mission_id": mission_id,
        }),
    });
}

fn advance_substrate_phase(state: &mut AppState) {
    state.substrate.advance_generation();
    state
        .substrate
        .prune_signals_below(crate::substrate::SubstrateState::DEFAULT_PRUNE_THRESHOLD);
    state
        .substrate
        .expire_claims(state.substrate.current_generation());
}

// Phase 5: observers run AFTER advance + prune. Emissions are frozen to a Vec
// first so no observer sees another observer's emissions within the same tick.
// Phase 6: arbiters run AFTER observers; `reduce_proposals` downgrades to
// `EmitSignalOnly` if we're already at the retry cap (mirror of nit-tui's
// GENOME_RETRY_LIMIT).
fn run_observers_and_arbiters(state: &mut AppState) {
    let emissions = crate::observers::run_all(state);
    for (observer_name, em) in emissions {
        let posted_by = format!("observer:{observer_name}");
        let id = state.substrate.next_signal_id(&posted_by);
        let posted_at_gen = state.substrate.current_generation();
        state.substrate.emit_signal(crate::substrate::Signal {
            id,
            kind: em.kind,
            posted_by,
            posted_at_gen,
            target: em.target,
            initial_strength: em.initial_strength,
            payload: em.payload,
        });
    }

    let raw = crate::arbiters::run_all(state);
    let reduced =
        crate::arbiters::reduce_proposals(state, raw, crate::arbiters::ARBITER_RETRY_LIMIT);
    crate::arbiters::apply_interventions(state, reduced);
}
