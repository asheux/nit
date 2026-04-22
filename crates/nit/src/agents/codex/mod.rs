mod cache;
mod metadata;

pub(in crate::agents) fn load_agents_from_codex_models_cache(
) -> anyhow::Result<nit_core::AgentsState> {
    let (path, entries) = cache::read_and_sort_entries()?;

    let mut agents = nit_core::AgentsState::default();
    agents.mcp.state = nit_core::McpConnectionState::Connected;
    agents.mcp.endpoint = format!("codex://cache ({})", path.display());
    agents.mcp.latency_ms = None;
    agents.mcp.last_error = None;

    metadata::populate_codex_metadata(&mut agents, &entries);
    agents.agents = metadata::build_codex_lanes(entries);

    agents.selected_agent = agents.agents.first().map(|lane| lane.id.clone());
    agents.roster_selected = 0;
    Ok(agents)
}

pub(in crate::agents) fn load_only_codex_agents() -> nit_core::AgentsState {
    load_agents_from_codex_models_cache().unwrap_or_else(|err| {
        let mut agents = nit_core::AgentsState::default();
        agents.alerts.push(nit_core::AgentAlert {
            severity: nit_core::AgentAlertSeverity::Warn,
            source: "codex".into(),
            message: format!("Failed to load Codex models: {err}"),
            at: "t+0".into(),
        });
        agents
    })
}
