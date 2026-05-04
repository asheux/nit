mod binary;
mod effort;
mod probe;

pub(super) use probe::probe_claude_models;

#[cfg(test)]
pub(crate) use binary::{parse_claude_models_from_binary, select_current_claude_models};
#[cfg(test)]
pub(crate) use effort::parse_effort_choices_from_help;

use probe::{EXTENDED_CONTEXT_WINDOW, STANDARD_CONTEXT_WINDOW};

pub(super) fn claude_lane() -> nit_core::AgentLane {
    nit_core::AgentLane {
        id: "claude".into(),
        role: "Claude".into(),
        lane: "Claude".into(),
        kind: nit_core::AgentLaneKind::Claude,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: "Claude backend detected.".into(),
        shadow: false,
    }
}

pub(super) fn load_only_claude_agents(cli_available: bool) -> nit_core::AgentsState {
    let mut agents = nit_core::AgentsState::default();
    if !cli_available {
        agents.alerts.push(nit_core::AgentAlert {
            severity: nit_core::AgentAlertSeverity::Warn,
            source: "claude".into(),
            message: "Claude CLI not found in PATH.".into(),
            at: "t+0".into(),
        });
        return agents;
    }
    agents.agents.push(claude_lane());
    agents.rebuild_agents_index();
    agents.selected_agent = Some("claude".into());
    agents.roster_selected = 0;
    agents
}

pub(super) fn populate_claude_model_metadata(roster: &mut nit_core::AgentsState) {
    let supported = probe::probe_claude_supported_efforts();
    let default_effort = effort::pick_claude_default_effort(&supported);

    for id in roster.claude_models.clone() {
        let window = if id.contains("[1m]") || id.contains("1m") {
            EXTENDED_CONTEXT_WINDOW
        } else {
            STANDARD_CONTEXT_WINDOW
        };
        roster
            .claude_effective_context_window_tokens
            .insert(id.clone(), window);

        roster
            .claude_supported_efforts
            .insert(id.clone(), supported.clone());
        roster
            .claude_default_effort
            .insert(id.clone(), default_effort.clone());
        roster
            .claude_selected_effort
            .insert(id, default_effort.clone());
    }
}
