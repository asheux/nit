mod binary;
mod probe;

pub(super) use probe::probe_claude_models;

#[cfg(test)]
pub(crate) use binary::{parse_claude_models_from_binary, select_current_claude_models};
#[cfg(test)]
pub(crate) use probe::parse_effort_choices_from_help;

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
    let metadata = build_claude_model_metadata(&roster.claude_models);
    roster.claude_effective_context_window_tokens = metadata.effective_context_window_tokens;
    roster.claude_supported_efforts = metadata.supported_efforts;
    roster.claude_default_effort = metadata.default_effort;
    roster.claude_selected_effort = metadata.selected_effort;
}

/// Compute the per-model metadata maps for the listed Claude models.
/// Shared by `populate_claude_model_metadata` (cache-hit init path) and
/// the async-probe spawner (the cache-miss path that emits a
/// `BackendModelsLoaded` event carrying the populated maps). Pre-fix
/// the latter path emitted only the model list, so the roster showed
/// model names with blank context-window cells until a restart picked
/// up the freshly written cache. The shared computation guarantees
/// both paths populate identically.
pub(super) fn build_claude_model_metadata(model_ids: &[String]) -> nit_core::BackendModelsMetadata {
    use std::collections::HashMap;
    let supported = probe::probe_claude_supported_efforts();
    let default_effort = probe::pick_claude_default_effort(&supported);

    let mut effective_context_window_tokens: HashMap<String, u32> = HashMap::new();
    let mut supported_efforts: HashMap<String, Vec<String>> = HashMap::new();
    let mut default_effort_map: HashMap<String, String> = HashMap::new();
    let mut selected_effort: HashMap<String, String> = HashMap::new();

    for id in model_ids {
        let window = if id.contains("[1m]") || id.contains("1m") {
            EXTENDED_CONTEXT_WINDOW
        } else {
            STANDARD_CONTEXT_WINDOW
        };
        effective_context_window_tokens.insert(id.clone(), window);
        supported_efforts.insert(id.clone(), supported.clone());
        default_effort_map.insert(id.clone(), default_effort.clone());
        selected_effort.insert(id.clone(), default_effort.clone());
    }

    nit_core::BackendModelsMetadata {
        effective_context_window_tokens,
        supported_efforts,
        default_effort: default_effort_map,
        selected_effort,
    }
}
