mod claude;
mod codex;
mod discover;
mod gemini;

#[cfg(test)]
pub(crate) use claude::{
    parse_claude_models_from_binary, parse_effort_choices_from_help, select_current_claude_models,
};
#[cfg(test)]
pub(crate) use gemini::{parse_gemini_models_from_source, select_current_gemini_models};

use nit_core::{
    AgentAlert, AgentAlertSeverity, AgentLane, AgentLaneKind, AgentStatus, AgentsState,
};

use crate::cli::AgentsArg;

const LOCAL_LANE_ID: &str = "local";

pub(crate) fn init_agents(backend_selection: AgentsArg) -> AgentsState {
    let codex_detected = discover::codex_cli_available();
    let claude_detected = discover::claude_cli_available();
    let gemini_detected = discover::gemini_cli_available();

    let mut roster = load_roster_for(backend_selection, codex_detected, claude_detected);
    roster.codex_cli_available = codex_detected;
    roster.claude_cli_available = claude_detected;
    roster.gemini_cli_available = gemini_detected;

    probe_claude_backend(backend_selection, &mut roster);
    probe_gemini_backend(backend_selection, &mut roster);
    sync_backend_model_lanes(&mut roster, backend_selection);

    roster
}

pub(crate) fn sync_backend_model_lanes(roster: &mut AgentsState, selection: AgentsArg) {
    let expand_claude =
        matches!(selection, AgentsArg::All | AgentsArg::Claude) && !roster.claude_models.is_empty();
    let expand_gemini = matches!(selection, AgentsArg::All) && !roster.gemini_models.is_empty();

    if !expand_claude && !expand_gemini {
        return;
    }

    let prior_selection = roster.selected_agent.clone();

    roster.agents.retain(|lane| {
        !((expand_claude && matches!(lane.kind, AgentLaneKind::Claude))
            || (expand_gemini && matches!(lane.kind, AgentLaneKind::Gemini)))
    });

    if expand_claude {
        materialize_model_lanes(
            &mut roster.agents,
            &roster.claude_models,
            "Claude",
            AgentLaneKind::Claude,
        );
    }
    if expand_gemini {
        materialize_model_lanes(
            &mut roster.agents,
            &roster.gemini_models,
            "Gemini",
            AgentLaneKind::Gemini,
        );
    }

    restore_selection_after_expansion(roster, prior_selection);
}

fn load_roster_for(backend: AgentsArg, codex_present: bool, claude_present: bool) -> AgentsState {
    match backend {
        AgentsArg::Local => assemble_local_roster(),
        AgentsArg::Codex => codex::load_only_codex_agents(),
        AgentsArg::Claude => claude::load_only_claude_agents(claude_present),
        AgentsArg::All => assemble_combined_roster(codex_present, claude_present),
    }
}

fn probe_claude_backend(selection: AgentsArg, roster: &mut AgentsState) {
    if !matches!(selection, AgentsArg::All | AgentsArg::Claude) || !roster.claude_cli_available {
        roster.claude_models.clear();
        roster.claude_models_error = None;
        return;
    }

    let (discovered_models, probe_error) = claude::probe_claude_models();
    roster.claude_models = discovered_models;
    roster.claude_models_error = probe_error;
    claude::populate_claude_model_metadata(roster);
}

fn probe_gemini_backend(selection: AgentsArg, roster: &mut AgentsState) {
    if !matches!(selection, AgentsArg::All) || !roster.gemini_cli_available {
        roster.gemini_models.clear();
        roster.gemini_models_error = None;
        return;
    }

    let (discovered_models, probe_error) = gemini::probe_gemini_models();
    roster.gemini_models = discovered_models;
    roster.gemini_models_error = probe_error;
}

fn materialize_model_lanes(
    lanes: &mut Vec<AgentLane>,
    models: &[String],
    display_name: &str,
    kind: AgentLaneKind,
) {
    for model_id in models {
        lanes.push(AgentLane {
            id: model_id.clone(),
            role: model_id.clone(),
            lane: display_name.into(),
            kind,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
            shadow: false,
        });
    }
}

fn restore_selection_after_expansion(roster: &mut AgentsState, prior: Option<String>) {
    if let Some(ref id) = prior {
        if let Some(pos) = roster.agents.iter().position(|lane| lane.id == *id) {
            roster.selected_agent = prior;
            roster.roster_selected = pos;
            return;
        }
    }
    roster.selected_agent = roster.agents.first().map(|lane| lane.id.clone());
    roster.roster_selected = 0;
}

fn assemble_local_roster() -> AgentsState {
    let local_lane = AgentLane {
        id: LOCAL_LANE_ID.into(),
        role: "Local".into(),
        lane: "Local".into(),
        kind: AgentLaneKind::Mock,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: "Built-in local lane.".into(),
        shadow: false,
    };

    let mut roster = AgentsState::default();
    roster.agents.push(local_lane);
    roster.selected_agent = Some(LOCAL_LANE_ID.into());
    roster.roster_selected = 0;
    roster
}

fn assemble_combined_roster(codex_available: bool, claude_available: bool) -> AgentsState {
    let mut roster = assemble_local_roster();

    if claude_available {
        roster.agents.push(claude::claude_lane());
    }
    if codex_available {
        incorporate_codex_cache(&mut roster);
    }

    roster.selected_agent = roster.agents.first().map(|lane| lane.id.clone());
    roster.roster_selected = 0;
    roster
}

fn incorporate_codex_cache(destination: &mut AgentsState) {
    let codex_state = match codex::load_agents_from_codex_models_cache() {
        Ok(state) => state,
        Err(err) => {
            destination.alerts.push(AgentAlert {
                severity: AgentAlertSeverity::Warn,
                source: "codex".into(),
                message: format!("Failed to load Codex models: {err}"),
                at: "t+0".into(),
            });
            return;
        }
    };

    apply_codex_fields(destination, codex_state);
}

fn apply_codex_fields(dest: &mut AgentsState, src: AgentsState) {
    dest.codex_default_reasoning_effort = src.codex_default_reasoning_effort;
    dest.codex_supported_reasoning_efforts = src.codex_supported_reasoning_efforts;
    dest.codex_selected_reasoning_effort = src.codex_selected_reasoning_effort;
    dest.codex_effective_context_window_tokens = src.codex_effective_context_window_tokens;
    dest.agents.extend(src.agents);
    dest.mcp = src.mcp;
}

fn prefer_shorter_model_name(challenger: &str, incumbent: &str) -> bool {
    (challenger.len(), challenger) < (incumbent.len(), incumbent)
}
