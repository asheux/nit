use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;

#[derive(Deserialize)]
struct CodexModelsCache {
    models: Vec<CodexModelEntry>,
}

#[derive(Deserialize)]
struct CodexModelEntry {
    slug: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    priority: Option<i64>,
    #[serde(default)]
    context_window: Option<u32>,
    #[serde(default)]
    effective_context_window_percent: Option<u8>,
    #[serde(default)]
    default_reasoning_level: Option<String>,
    #[serde(default)]
    supported_reasoning_levels: Option<Vec<CodexReasoningLevel>>,
}

#[derive(Deserialize)]
struct CodexReasoningLevel {
    effort: String,
}

pub(super) fn load_agents_from_codex_models_cache() -> anyhow::Result<nit_core::AgentsState> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let path = PathBuf::from(home).join(".codex").join("models_cache.json");
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let cache: CodexModelsCache =
        serde_json::from_str(&raw).context("parse ~/.codex/models_cache.json")?;

    let mut entries: Vec<_> = cache
        .models
        .into_iter()
        .filter(|m| m.visibility.as_deref().unwrap_or("list") == "list")
        .collect();
    entries.sort_by(|a, b| {
        a.priority
            .unwrap_or(i64::MAX)
            .cmp(&b.priority.unwrap_or(i64::MAX))
            .then_with(|| a.slug.cmp(&b.slug))
    });

    let mut agents = nit_core::AgentsState::default();
    agents.mcp.state = nit_core::McpConnectionState::Connected;
    agents.mcp.endpoint = format!("codex://cache ({})", path.display());
    agents.mcp.latency_ms = None;
    agents.mcp.last_error = None;

    for entry in &entries {
        if let Some(window) = entry.context_window {
            let pct = entry.effective_context_window_percent.unwrap_or(100) as u64;
            let tokens = (window as u64).saturating_mul(pct).saturating_div(100) as u32;
            agents
                .codex_effective_context_window_tokens
                .insert(entry.slug.clone(), tokens.max(1));
        }

        if let Some(levels) = entry.supported_reasoning_levels.as_ref() {
            let mut efforts: Vec<String> = levels
                .iter()
                .map(|lvl| lvl.effort.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            efforts.sort_by(|a, b| {
                reasoning_effort_rank(a)
                    .cmp(&reasoning_effort_rank(b))
                    .then_with(|| a.cmp(b))
            });
            efforts.dedup();
            if !efforts.is_empty() {
                agents
                    .codex_supported_reasoning_efforts
                    .insert(entry.slug.clone(), efforts);
            }
        }

        if let Some(effort) = pick_codex_reasoning_effort(entry) {
            agents
                .codex_default_reasoning_effort
                .insert(entry.slug.clone(), effort.clone());
            agents
                .codex_selected_reasoning_effort
                .insert(entry.slug.clone(), effort);
        }
    }

    agents.agents = entries
        .into_iter()
        .map(|entry| nit_core::AgentLane {
            id: entry.slug.clone(),
            role: entry
                .display_name
                .clone()
                .unwrap_or_else(|| entry.slug.clone()),
            lane: "Codex".into(),
            kind: nit_core::AgentLaneKind::Codex,
            status: nit_core::AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: entry.description.unwrap_or_default(),
        })
        .collect();

    agents.selected_agent = agents.agents.first().map(|lane| lane.id.clone());
    agents.roster_selected = 0;
    Ok(agents)
}

pub(super) fn load_only_codex_agents() -> nit_core::AgentsState {
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

fn reasoning_effort_rank(effort: &str) -> u8 {
    match effort.to_ascii_lowercase().as_str() {
        "low" => 0,
        "medium" => 1,
        "high" => 2,
        "xhigh" => 3,
        _ => 10,
    }
}

fn pick_codex_reasoning_effort(model: &CodexModelEntry) -> Option<String> {
    let default = model
        .default_reasoning_level
        .as_deref()
        .unwrap_or("medium")
        .trim();

    let Some(levels) = model.supported_reasoning_levels.as_ref() else {
        return Some(default.to_string());
    };

    let supported: Vec<&str> = levels
        .iter()
        .map(|lvl| lvl.effort.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if supported.is_empty() {
        return Some(default.to_string());
    }

    let find = |target: &str| {
        supported
            .iter()
            .find(|e| e.eq_ignore_ascii_case(target))
            .copied()
    };

    // Prefer the model's default, then common tiers, then whatever is first.
    let chosen = find(default)
        .or_else(|| find("medium"))
        .or_else(|| find("high"))
        .or_else(|| find("low"))
        .unwrap_or(supported[0]);

    Some(chosen.to_string())
}
