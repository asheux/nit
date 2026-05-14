mod cache;
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
    // CLI availability is cheap (a few `which`-style PATH walks). Keep it
    // synchronous so the rest of init has accurate booleans to branch on.
    let codex_path = discover::find_executable_in_path("codex");
    let claude_path = discover::find_executable_in_path("claude");
    let gemini_path = discover::find_executable_in_path("gemini");
    let codex_detected = codex_path.is_some();
    let claude_detected = claude_path.is_some();
    let gemini_detected = gemini_path.is_some();

    let mut roster = load_roster_for(backend_selection, codex_detected, claude_detected);
    roster.codex_cli_available = codex_detected;
    roster.claude_cli_available = claude_detected;
    roster.gemini_cli_available = gemini_detected;

    // Whether each backend is in scope for this `--agents` selection. A
    // backend out-of-scope is treated as "no models", same as today — we
    // don't probe or cache it.
    let want_claude =
        matches!(backend_selection, AgentsArg::All | AgentsArg::Claude) && claude_detected;
    let want_gemini = matches!(backend_selection, AgentsArg::All) && gemini_detected;

    // Try to skip the subprocess probes via the on-disk cache. Two miss
    // conditions: TTL expired (`is_fresh`) or the resolved binary path
    // changed since the last cache write (`binaries_match`). On hit, we
    // populate the roster from disk in ~10 ms and return; on miss we
    // probe in parallel and update the cache.
    let now = cache::now_unix();
    let cache_hit = cache::load().filter(|cached| {
        cached.is_fresh(now)
            && cached.binaries_match(claude_path.as_deref(), gemini_path.as_deref())
    });

    if let Some(cached) = cache_hit {
        if want_claude {
            roster.claude_models = cached.claude_models.clone();
            roster.claude_models_error = cached.claude_models_error.clone();
            claude::populate_claude_model_metadata(&mut roster);
        }
        if want_gemini {
            roster.gemini_models = cached.gemini_models.clone();
            roster.gemini_models_error = cached.gemini_models_error.clone();
        }
        sync_backend_model_lanes(&mut roster, backend_selection);
        return roster;
    }

    // Cache miss / stale: spawn the probe in the background so the TUI can
    // paint immediately. The thread emits `BackendModelsLoaded` events on
    // the shared async queue when each probe finishes; the event loop
    // drains the queue and applies the events, replacing the placeholder
    // lanes with the discovered ones and clearing the loading flag.
    roster.claude_models_loading = want_claude;
    roster.gemini_models_loading = want_gemini;
    spawn_background_probe(BackgroundProbeArgs {
        want_claude,
        want_gemini,
        claude_path,
        gemini_path,
    });
    sync_backend_model_lanes(&mut roster, backend_selection);
    roster
}

struct BackgroundProbeArgs {
    want_claude: bool,
    want_gemini: bool,
    claude_path: Option<std::path::PathBuf>,
    gemini_path: Option<std::path::PathBuf>,
}

fn spawn_background_probe(args: BackgroundProbeArgs) {
    if !args.want_claude && !args.want_gemini {
        return;
    }
    std::thread::Builder::new()
        .name("nit-agents-probe".into())
        .spawn(move || {
            // Both probes run in parallel sub-threads (when both are in
            // scope) so their subprocess time overlaps. Cache write
            // happens once both finish.
            let claude_handle = args
                .want_claude
                .then(|| std::thread::spawn(claude::probe_claude_models));
            let gemini_handle = args
                .want_gemini
                .then(|| std::thread::spawn(gemini::probe_gemini_models));

            let (claude_models, claude_error) = match claude_handle {
                Some(h) => h.join().unwrap_or_else(|_| {
                    (Vec::new(), Some("claude probe thread panicked".to_string()))
                }),
                None => (Vec::new(), None),
            };
            let (gemini_models, gemini_error) = match gemini_handle {
                Some(h) => h.join().unwrap_or_else(|_| {
                    (Vec::new(), Some("gemini probe thread panicked".to_string()))
                }),
                None => (Vec::new(), None),
            };

            // Persist for the next launch.
            cache::save(&cache::ProbeCache::new(
                args.claude_path.as_deref(),
                claude_models.clone(),
                claude_error.clone(),
                args.gemini_path.as_deref(),
                gemini_models.clone(),
                gemini_error.clone(),
            ));

            // Push events into the async queue for the event loop to drain.
            // Emit one event per backend that was probed so the loader
            // clears as soon as each finishes (Claude usually returns
            // first; Gemini's CLI is heavier).
            if args.want_claude {
                nit_core::agent_bus::async_queue::push(
                    nit_core::AgentBusEvent::BackendModelsLoaded {
                        backend: nit_core::BackendKind::Claude,
                        models: claude_models,
                        error: claude_error,
                    },
                );
            }
            if args.want_gemini {
                nit_core::agent_bus::async_queue::push(
                    nit_core::AgentBusEvent::BackendModelsLoaded {
                        backend: nit_core::BackendKind::Gemini,
                        models: gemini_models,
                        error: gemini_error,
                    },
                );
            }
        })
        .expect("failed to spawn nit-agents-probe thread");
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
    let src = match codex::load_agents_from_codex_models_cache() {
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

    destination.codex_default_reasoning_effort = src.codex_default_reasoning_effort;
    destination.codex_supported_reasoning_efforts = src.codex_supported_reasoning_efforts;
    destination.codex_selected_reasoning_effort = src.codex_selected_reasoning_effort;
    destination.codex_effective_context_window_tokens = src.codex_effective_context_window_tokens;
    destination.agents.extend(src.agents);
    destination.mcp = src.mcp;
}

fn prefer_shorter_model_name(challenger: &str, incumbent: &str) -> bool {
    (challenger.len(), challenger) < (incumbent.len(), incumbent)
}
