use super::cache::CodexModelEntry;

pub(super) fn populate_codex_metadata(
    agents: &mut nit_core::AgentsState,
    entries: &[CodexModelEntry],
) {
    for entry in entries {
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
}

pub(super) fn build_codex_lanes(entries: Vec<CodexModelEntry>) -> Vec<nit_core::AgentLane> {
    entries
        .into_iter()
        .map(|entry| {
            let role = entry.display_name.unwrap_or_else(|| entry.slug.clone());
            let description = entry.description.unwrap_or_default();
            nit_core::AgentLane {
                id: entry.slug,
                role,
                lane: "Codex".into(),
                kind: nit_core::AgentLaneKind::Codex,
                status: nit_core::AgentStatus::Idle,
                heartbeat_age_secs: 0,
                queue_len: 0,
                current_mission: None,
                last_message: description,
                shadow: false,
            }
        })
        .collect()
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

    let chosen = find(default)
        .or_else(|| find("medium"))
        .or_else(|| find("high"))
        .or_else(|| find("low"))
        .unwrap_or(supported[0]);

    Some(chosen.to_string())
}
