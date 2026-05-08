use crate::state::{
    AgentAlert, AgentAlertSeverity, AgentDiagnosticEvent, AgentStatus, AppState,
    CONSOLE_SCROLL_BOTTOM,
};

use super::helpers::{
    backend_source_for_agent, is_runner_internal_cancel, summarize_agent_error, timestamp_label,
};
use super::token_count::apply_codex_token_count;
use super::AgentTokenCount;
use super::OPERATOR_CANCEL_TURN_MESSAGE;

pub(super) fn handle_turn_failed(
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

    // Operator-initiated cancels (`/abort`, Ctrl+C, Esc-Esc, Mission-tab `x`)
    // and runner-internal cancels (MCP stop, reconnect) ride the same
    // TurnFailed event because they kill the subprocess, but they're NOT
    // errors. Route them down the soft path: Idle status, Info diag, no
    // substrate warning, no "Codex failed: …" status banner.
    let is_operator_cancel = message == OPERATOR_CANCEL_TURN_MESSAGE;
    let is_soft_cancel = is_operator_cancel || is_runner_internal_cancel(message);
    let new_status = if is_soft_cancel {
        AgentStatus::Idle
    } else {
        AgentStatus::Error
    };

    if let Some(agent) = state.agents.agents_get_mut(agent_id) {
        agent.status = new_status;
        agent.queue_len = agent.queue_len.saturating_sub(1);
        agent.heartbeat_age_secs = 0;
        agent.current_mission = mission_id.clone();
    }

    record_thread_id(state, agent_id, mission_id.as_deref(), thread_id.as_deref());
    update_mission_status_on_failure(state, mission_id.as_deref(), is_soft_cancel, &at);

    if is_soft_cancel {
        push_soft_cancel_diag(state, agent_id, source, message, is_operator_cancel, &at);
    } else {
        push_error_records(state, agent_id, source, message, thread_id, mission_id, &at);
    }

    // TurnFailed does not advance the generation, but we still clear expired
    // claims so stale holds don't linger when turns keep failing.
    state
        .substrate
        .expire_claims(state.substrate.current_generation());
}

fn record_thread_id(
    state: &mut AppState,
    agent_id: &str,
    mission_id: Option<&str>,
    thread_id: Option<&str>,
) {
    let Some(thread_id) = thread_id else {
        return;
    };
    let entry = (agent_id.to_string(), thread_id.to_string());
    if let Some(mission_id) = mission_id {
        state
            .agents
            .codex_mission_thread_ids
            .entry(mission_id.to_string())
            .or_default()
            .insert(entry.0, entry.1);
        return;
    }
    state.agents.codex_thread_ids.insert(entry.0, entry.1);
}

fn update_mission_status_on_failure(
    state: &mut AppState,
    mission_id: Option<&str>,
    is_soft_cancel: bool,
    at: &str,
) {
    let Some(mission_id) = mission_id else {
        return;
    };
    let Some(mission) = state
        .agents
        .missions
        .iter_mut()
        .find(|mission| mission.id == mission_id)
    else {
        return;
    };
    // Don't clobber "ABORTED" with "ERROR" when this TurnFailed is the
    // abort itself OR when an MCP server exit/disconnect races with
    // /abort all. The swarm runtime sets "ABORTED" before the runner's
    // cancel TurnFailed reaches us.
    if !is_soft_cancel && mission.status != "ABORTED" {
        mission.status = "ERROR".into();
    }
    mission.updated_at = at.to_string();
}

fn push_soft_cancel_diag(
    state: &mut AppState,
    agent_id: &str,
    source: &'static str,
    message: &str,
    is_operator_cancel: bool,
    at: &str,
) {
    // Soft path: a single Info diag is enough — for operator cancels the swarm
    // runtime already pushed a SYSTEM_ALERT_KIND chat message explaining the
    // abort. No alert, no signal, no error banner.
    let diag_msg = if is_operator_cancel {
        format!("[{agent_id}] cancelled by operator")
    } else {
        format!("[{agent_id}] {message}")
    };
    state.agents.diag_events.push(AgentDiagnosticEvent {
        severity: AgentAlertSeverity::Info,
        source: source.into(),
        message: diag_msg,
        at: at.to_string(),
    });
}

fn push_error_records(
    state: &mut AppState,
    agent_id: &str,
    source: &'static str,
    message: &str,
    thread_id: &Option<String>,
    mission_id: &Option<String>,
    at: &str,
) {
    let source_label = match source {
        "claude" => "Claude",
        "gemini" => "Gemini",
        "local" => "Local",
        _ => "Codex",
    };
    state.agents.alerts.push(AgentAlert {
        severity: AgentAlertSeverity::Error,
        source: source.into(),
        message: format!("[{agent_id}] {message}"),
        at: at.to_string(),
    });
    state.agents.diag_events.push(AgentDiagnosticEvent {
        severity: AgentAlertSeverity::Error,
        source: source.into(),
        message: format!("[{agent_id}] {message}"),
        at: at.to_string(),
    });
    state.agents.console_scroll = CONSOLE_SCROLL_BOTTOM;

    let current_gen = state.substrate.current_generation();
    let id = state.substrate.next_signal_id(agent_id);
    state.substrate.emit_signal(crate::substrate::Signal {
        id,
        kind: crate::substrate::SignalKind::Warning,
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
    let _ = state.substrate.save(&state.workspace_root);

    state.status = Some(format!(
        "{source_label} failed: {}",
        summarize_agent_error(message)
    ));
}
