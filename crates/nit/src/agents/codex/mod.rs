mod cache;
mod metadata;

pub(in crate::agents) fn load_agents_from_codex_models_cache(
) -> anyhow::Result<nit_core::AgentsState> {
    let (path, entries) = cache::read_and_sort_entries()?;

    let mut state = nit_core::AgentsState::default();
    state.mcp.state = nit_core::McpConnectionState::Connected;
    state.mcp.endpoint = format!("codex://cache ({})", path.display());
    state.mcp.latency_ms = None;
    state.mcp.last_error = None;

    metadata::populate_codex_metadata(&mut state, &entries);
    state.agents = metadata::build_codex_lanes(entries);
    state.rebuild_agents_index();

    state.selected_agent = state.agents.first().map(|lane| lane.id.clone());
    state.roster_selected = 0;
    Ok(state)
}

pub(in crate::agents) fn load_only_codex_agents() -> nit_core::AgentsState {
    load_agents_from_codex_models_cache().unwrap_or_else(|err| {
        let mut state = nit_core::AgentsState::default();
        state.alerts.push(nit_core::AgentAlert {
            severity: nit_core::AgentAlertSeverity::Warn,
            source: "codex".into(),
            message: format!("Failed to load Codex models: {err}"),
            at: "t+0".into(),
        });
        state
    })
}
