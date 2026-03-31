//! Agent roster initialization: discovers installed CLI backends, probes model catalogs,
//! and materializes per-model lanes for the TUI agent console.

mod claude;
mod codex;
mod discover;
mod gemini;

#[cfg(test)]
pub(crate) use claude::{parse_claude_models_from_binary, select_current_claude_models};
#[cfg(test)]
pub(crate) use gemini::{parse_gemini_models_from_source, select_current_gemini_models};

use nit_core::{
    AgentAlert, AgentAlertSeverity, AgentLane, AgentLaneKind, AgentStatus, AgentsState,
};

use crate::cli::AgentsArg;

/// Status message for freshly materialized model lanes.
const IDLE_LANE_MESSAGE: &str = "";

/// Default local lane identifier.
const LOCAL_LANE_ID: &str = "local";

/// Initialize the complete agent roster by detecting CLI tooling, probing backends,
/// and expanding placeholder lanes into per-model lanes.
/// Discover installed CLI tooling, build the agent roster, probe model catalogs,
/// and expand placeholder lanes into per-model lanes.
pub(crate) fn init_agents(backend_selection: AgentsArg) -> AgentsState {
    // Phase 1: discover which CLI tools are installed.
    let codex_detected = discover::codex_cli_available();
    let claude_detected = discover::claude_cli_available();
    let gemini_detected = discover::gemini_cli_available();

    // Phase 2: build the skeleton roster from the user's backend selection.
    let mut assembled_roster =
        load_roster_for(backend_selection, codex_detected, claude_detected);
    assembled_roster.codex_cli_available = codex_detected;
    assembled_roster.claude_cli_available = claude_detected;
    assembled_roster.gemini_cli_available = gemini_detected;

    // Phase 3: probe backends for model catalogs and populate metadata.
    probe_claude_backend(backend_selection, &mut assembled_roster);
    probe_gemini_backend(backend_selection, &mut assembled_roster);

    // Phase 4: expand placeholder lanes into per-model lanes.
    sync_backend_model_lanes(&mut assembled_roster, backend_selection);

    assembled_roster
}

/// Replace placeholder backend lanes with per-model lanes from discovered catalogs.
///
/// When a backend probe found models, the single placeholder lane (e.g. "Claude")
/// is removed and replaced with one lane per discovered model identifier.
pub(crate) fn sync_backend_model_lanes(roster: &mut AgentsState, selection: AgentsArg) {
    // Determine which backend catalogs have discovered models to expand.
    let expanding_claude_catalog = matches!(selection, AgentsArg::All | AgentsArg::Claude)
        && !roster.claude_models.is_empty();

    let expanding_gemini_catalog =
        matches!(selection, AgentsArg::All) && !roster.gemini_models.is_empty();

    if !expanding_claude_catalog && !expanding_gemini_catalog {
        return;
    }

    let previously_selected_agent = roster.selected_agent.clone();

    // Filter out placeholder lanes that will be replaced by per-model lanes.
    let retained_lanes = filter_replaced_placeholders(
        roster.agents.drain(..).collect(),
        expanding_claude_catalog,
        expanding_gemini_catalog,
    );
    roster.agents = retained_lanes;

    // Materialize individual lanes for each discovered Claude model.
    if expanding_claude_catalog {
        materialize_model_lanes(
            &mut roster.agents,
            &roster.claude_models,
            "Claude",
            AgentLaneKind::Claude,
        );
    }

    // Materialize individual lanes for each discovered Gemini model.
    if expanding_gemini_catalog {
        materialize_model_lanes(
            &mut roster.agents,
            &roster.gemini_models,
            "Gemini",
            AgentLaneKind::Gemini,
        );
    }

    restore_selection_after_expansion(roster, previously_selected_agent);
}

// ── Initialization Sequence ──

/// Build the initial skeleton roster based on the user's CLI backend selection flag.
fn load_roster_for(
    requested_backend: AgentsArg,
    codex_present: bool,
    claude_present: bool,
) -> AgentsState {
    match requested_backend {
        AgentsArg::Local => assemble_local_roster(),
        AgentsArg::Codex => codex::load_only_codex_agents(),
        AgentsArg::Claude => claude::load_only_claude_agents(claude_present),
        AgentsArg::All => assemble_combined_roster(codex_present, claude_present),
    }
}

/// Attempt to discover Claude models when that backend was requested and is installed.
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

/// Attempt to discover Gemini models when the combined backend was requested.
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

// ── Lane Filtering and Expansion ──

/// Remove placeholder lanes for backends being expanded, keeping all others intact.
fn filter_replaced_placeholders(
    all_lanes: Vec<AgentLane>,
    replacing_claude: bool,
    replacing_gemini: bool,
) -> Vec<AgentLane> {
    all_lanes
        .into_iter()
        .filter(|lane| !is_expansion_target(lane, replacing_claude, replacing_gemini))
        .collect()
}

/// Determine whether a lane is a placeholder scheduled for per-model replacement.
fn is_expansion_target(
    lane_candidate: &AgentLane,
    claude_replacement_active: bool,
    gemini_replacement_active: bool,
) -> bool {
    (claude_replacement_active && matches!(lane_candidate.kind, AgentLaneKind::Claude))
        || (gemini_replacement_active && matches!(lane_candidate.kind, AgentLaneKind::Gemini))
}

/// Materialize one agent lane per discovered model identifier for a backend family.
fn materialize_model_lanes(
    lane_roster: &mut Vec<AgentLane>,
    model_catalog: &[String],
    backend_display_name: &str,
    lane_classification: AgentLaneKind,
) {
    for catalog_entry in model_catalog {
        lane_roster.push(AgentLane {
            id: catalog_entry.clone(),
            role: catalog_entry.clone(),
            lane: backend_display_name.into(),
            kind: lane_classification,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: IDLE_LANE_MESSAGE.into(),
        });
    }
}

/// After lane expansion, restore the user's prior agent selection if it still exists.
fn restore_selection_after_expansion(
    roster: &mut AgentsState,
    previously_selected: Option<String>,
) {
    let matching_roster_index = previously_selected
        .as_ref()
        .and_then(|target_id| roster.agents.iter().position(|lane| lane.id == *target_id));

    if let (Some(confirmed_selection), Some(confirmed_index)) =
        (previously_selected, matching_roster_index)
    {
        roster.selected_agent = Some(confirmed_selection);
        roster.roster_selected = confirmed_index;
        return;
    }

    roster.selected_agent = roster.agents.first().map(|first_lane| first_lane.id.clone());
    roster.roster_selected = 0;
}

// ── Roster Assembly ──

/// Assemble a local-only roster containing a single built-in mock lane.
fn assemble_local_roster() -> AgentsState {
    let builtin_local_lane = AgentLane {
        id: LOCAL_LANE_ID.into(),
        role: "Local".into(),
        lane: "Local".into(),
        kind: AgentLaneKind::Mock,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: "Built-in local lane.".into(),
    };

    let mut local_only_roster = AgentsState::default();
    local_only_roster.agents.push(builtin_local_lane);
    local_only_roster.selected_agent = Some(LOCAL_LANE_ID.into());
    local_only_roster.roster_selected = 0;
    local_only_roster
}

/// Assemble a comprehensive roster merging local, Claude, and Codex backends together.
fn assemble_combined_roster(codex_available: bool, claude_available: bool) -> AgentsState {
    let mut merged_roster = AgentsState::default();
    merged_roster.agents.extend(assemble_local_roster().agents);

    if claude_available {
        merged_roster.agents.push(claude::claude_lane());
    }

    if codex_available {
        incorporate_codex_cache(&mut merged_roster);
    }

    merged_roster.selected_agent = merged_roster.agents.first().map(|first_lane| first_lane.id.clone());
    merged_roster.roster_selected = 0;
    merged_roster
}

/// Load the Codex models cache and incorporate its metadata into the destination roster.
///
/// On failure, a warning alert is pushed rather than aborting the full initialization.
fn incorporate_codex_cache(destination: &mut AgentsState) {
    let codex_agents_data = match codex::load_agents_from_codex_models_cache() {
        Ok(parsed_cache) => parsed_cache,
        Err(load_failure) => {
            destination.alerts.push(AgentAlert {
                severity: AgentAlertSeverity::Warn,
                source: "codex".into(),
                message: format!("Failed to load Codex models: {load_failure}"),
                at: "t+0".into(),
            });
            return;
        }
    };

    apply_codex_fields(destination, codex_agents_data);
}

/// Transfer Codex-specific configuration fields from the loaded cache into the target roster.
fn apply_codex_fields(destination: &mut AgentsState, codex_snapshot: AgentsState) {
    transfer_reasoning_fields(destination, &codex_snapshot);
    destination.codex_effective_context_window_tokens =
        codex_snapshot.codex_effective_context_window_tokens;
    destination.agents.extend(codex_snapshot.agents);
    destination.mcp = codex_snapshot.mcp;
}

/// Copy the three reasoning-effort fields from a Codex snapshot into the destination.
fn transfer_reasoning_fields(destination: &mut AgentsState, source: &AgentsState) {
    destination.codex_default_reasoning_effort = source.codex_default_reasoning_effort.clone();
    destination.codex_supported_reasoning_efforts =
        source.codex_supported_reasoning_efforts.clone();
    destination.codex_selected_reasoning_effort = source.codex_selected_reasoning_effort.clone();
}

// ── Model Name Comparison ──

/// Tiebreak between two model identifiers: prefer the shorter, or lexicographically first.
fn prefer_shorter_model_name(challenger_name: &str, current_best_name: &str) -> bool {
    let challenger_len = challenger_name.len();
    let current_best_len = current_best_name.len();
    challenger_len < current_best_len
        || (challenger_len == current_best_len && challenger_name < current_best_name)
}
