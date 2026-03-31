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

    let mut entries = cache
        .models
        .into_iter()
        .filter(|m| m.visibility.as_deref().unwrap_or("list") == "list")
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        let pa = a.priority.unwrap_or(i64::MAX);
        let pb = b.priority.unwrap_or(i64::MAX);
        pa.cmp(&pb).then_with(|| a.slug.cmp(&b.slug))
    });

    let mut agents = nit_core::AgentsState::default();
    agents.mcp.state = nit_core::McpConnectionState::Connected;
    agents.mcp.endpoint = format!("codex://cache ({})", path.display());
    agents.mcp.latency_ms = None;
    agents.mcp.last_error = None;

    for model in entries.iter() {
        if let Some(context_window) = model.context_window {
            let effective_pct = model.effective_context_window_percent.unwrap_or(100) as u64;
            let effective_tokens = (context_window as u64)
                .saturating_mul(effective_pct)
                .saturating_div(100) as u32;
            agents
                .codex_effective_context_window_tokens
                .insert(model.slug.clone(), effective_tokens.max(1));
        }

        if let Some(levels) = model.supported_reasoning_levels.as_ref() {
            let mut efforts = levels
                .iter()
                .map(|lvl| lvl.effort.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            efforts.sort_by(|a, b| {
                reasoning_effort_rank(a)
                    .cmp(&reasoning_effort_rank(b))
                    .then_with(|| a.cmp(b))
            });
            efforts.dedup();
            if !efforts.is_empty() {
                agents
                    .codex_supported_reasoning_efforts
                    .insert(model.slug.clone(), efforts);
            }
        }

        if let Some(effort) = pick_codex_reasoning_effort(model) {
            agents
                .codex_default_reasoning_effort
                .insert(model.slug.clone(), effort.clone());
            agents
                .codex_selected_reasoning_effort
                .insert(model.slug.clone(), effort);
        }
    }

    agents.agents = entries
        .into_iter()
        .map(|model| nit_core::AgentLane {
            id: model.slug.clone(),
            role: model
                .display_name
                .clone()
                .unwrap_or_else(|| model.slug.clone()),
            lane: "Codex".into(),
            kind: nit_core::AgentLaneKind::Codex,
            status: nit_core::AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: model.description.unwrap_or_default(),
        })
        .collect();

    agents.selected_agent = agents.agents.first().map(|a| a.id.clone());
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
    if effort.eq_ignore_ascii_case("low") {
        0
    } else if effort.eq_ignore_ascii_case("medium") {
        1
    } else if effort.eq_ignore_ascii_case("high") {
        2
    } else if effort.eq_ignore_ascii_case("xhigh") {
        3
    } else {
        10
    }
}

fn pick_codex_reasoning_effort(model: &CodexModelEntry) -> Option<String> {
    let supported = model
        .supported_reasoning_levels
        .as_ref()
        .map(|levels| {
            levels
                .iter()
                .map(|lvl| lvl.effort.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let default = model
        .default_reasoning_level
        .as_deref()
        .unwrap_or("medium")
        .trim();
    if supported.is_empty() {
        return Some(default.to_string());
    }

    if let Some(found) = supported
        .iter()
        .find(|effort| effort.eq_ignore_ascii_case(default))
    {
        return Some((*found).to_string());
    }
    for effort in ["medium", "high", "low"] {
        if let Some(found) = supported
            .iter()
            .find(|candidate| candidate.eq_ignore_ascii_case(effort))
        {
            return Some((*found).to_string());
        }
    }

    supported
        .first()
        .copied()
        .map(str::to_string)
        .or_else(|| Some(default.to_string()))
}
