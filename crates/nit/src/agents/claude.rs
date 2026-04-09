//! Claude backend: model discovery via CLI probing and binary inspection,
//! family-based version selection, context window and effort metadata.

use std::collections::HashMap;
use std::fs;

use super::discover::{find_executable_in_path, probe_models_from_cli};

const STANDARD_CONTEXT_WINDOW: u32 = 200_000;
const EXTENDED_CONTEXT_WINDOW: u32 = 1_000_000;
const MIN_ASCII_RUN_LENGTH: usize = 8;

type ModelProbeResult = (Vec<String>, Option<String>);

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
    agents.selected_agent = Some("claude".into());
    agents.roster_selected = 0;
    agents
}

/// Try CLI-based discovery first, fall back to scanning the installed binary.
pub(super) fn probe_claude_models() -> ModelProbeResult {
    let (cli_raw_output, cli_error) = probe_models_from_cli(
        "claude",
        &[
            &["models", "--json"],
            &["models"],
            &["list-models"],
            &["--list-models"],
        ],
    );

    let cli_filtered = select_current_claude_models(cli_raw_output);
    if !cli_filtered.is_empty() {
        return (cli_filtered, None);
    }

    if let Some(binary_candidates) = probe_claude_models_from_install() {
        let binary_filtered = select_current_claude_models(binary_candidates);
        return (binary_filtered, None);
    }

    (cli_filtered, cli_error)
}

pub(super) fn populate_claude_model_metadata(roster: &mut nit_core::AgentsState) {
    for id in roster.claude_models.clone() {
        let window = if id.contains("[1m]") || id.contains("1m") {
            EXTENDED_CONTEXT_WINDOW
        } else {
            STANDARD_CONTEXT_WINDOW
        };
        roster
            .claude_effective_context_window_tokens
            .insert(id.clone(), window);

        let tiers = if id.to_lowercase().contains("opus") {
            vec!["low".into(), "medium".into(), "high".into(), "max".into()]
        } else {
            vec!["low".into(), "medium".into(), "high".into()]
        };
        roster.claude_supported_efforts.insert(id.clone(), tiers);
        roster
            .claude_default_effort
            .insert(id.clone(), "high".into());
        roster
            .claude_selected_effort
            .insert(id.clone(), "high".into());
    }
}

pub(crate) fn parse_claude_models_from_binary(bytes: &[u8]) -> Vec<String> {
    let fragments = extract_ascii_runs(bytes);

    let mut models = Vec::new();
    for pair in fragments.windows(2) {
        let Some(model) = normalize_claude_model_token(&pair[0]) else {
            continue;
        };
        if looks_like_claude_model_label(&pair[1]) {
            models.push(model.to_string());
        }
    }
    models.sort();
    models.dedup();
    models
}

/// Keep only the latest version per model family; returns all if no families recognized.
pub(crate) fn select_current_claude_models(models: Vec<String>) -> Vec<String> {
    let mut deduped = models;
    deduped.sort();
    deduped.dedup();

    let mut best_per_family: HashMap<&'static str, (Vec<u32>, String)> = HashMap::new();
    for model in deduped.iter() {
        let Some((family, version)) = parse_claude_family_and_version(model) else {
            continue;
        };
        update_latest_per_family(&mut best_per_family, family, version, model);
    }

    if best_per_family.is_empty() {
        return deduped;
    }

    let mut result: Vec<String> = best_per_family
        .into_values()
        .map(|(_, model)| model)
        .collect();
    result.sort();
    result
}

fn update_latest_per_family(
    map: &mut HashMap<&'static str, (Vec<u32>, String)>,
    family: &'static str,
    version: Vec<u32>,
    model: &str,
) {
    let dominated = map.get(family).is_some_and(|(inc_ver, inc_name)| {
        version < *inc_ver
            || (version == *inc_ver && !super::prefer_shorter_model_name(model, inc_name))
    });
    if !dominated {
        map.insert(family, (version, model.to_owned()));
    }
}

fn probe_claude_models_from_install() -> Option<Vec<String>> {
    let executable = find_executable_in_path("claude")?;
    let bytes = fs::read(executable).ok()?;
    let models = parse_claude_models_from_binary(&bytes);
    if models.is_empty() {
        None
    } else {
        Some(models)
    }
}

/// Collect contiguous printable ASCII runs of at least `MIN_ASCII_RUN_LENGTH` bytes.
fn extract_ascii_runs(bytes: &[u8]) -> Vec<String> {
    if bytes.is_empty() {
        return Vec::new();
    }

    let mut runs = Vec::new();
    let mut start: Option<usize> = None;

    for (i, &b) in bytes.iter().enumerate() {
        if b.is_ascii_graphic() || b == b' ' {
            if start.is_none() {
                start = Some(i);
            }
            continue;
        }
        if let Some(begin) = start.take() {
            if i - begin >= MIN_ASCII_RUN_LENGTH {
                runs.push(String::from_utf8_lossy(&bytes[begin..i]).into_owned());
            }
        }
    }

    if let Some(begin) = start {
        if bytes.len() - begin >= MIN_ASCII_RUN_LENGTH {
            runs.push(String::from_utf8_lossy(&bytes[begin..]).into_owned());
        }
    }

    runs
}

fn normalize_claude_model_token(raw: &str) -> Option<&str> {
    let stripped = raw.trim().strip_suffix("[1m]").unwrap_or(raw.trim());
    if is_probable_claude_model(stripped) {
        Some(stripped)
    } else {
        None
    }
}

const RECOGNIZED_FAMILIES: &[&str] = &["-haiku", "-sonnet", "-opus"];

const DISQUALIFYING_KEYWORDS: &[&str] = &[
    "api",
    "sdk",
    "cli",
    "code",
    "plugin",
    "desktop",
    "chrome",
    "agent",
    "guide",
    "github",
    "review",
    "marketplace",
    "settings",
    "context",
    "swarm",
    "folder",
    "hidden",
    "http",
    "staging",
];

fn is_probable_claude_model(raw: &str) -> bool {
    let name = raw.to_ascii_lowercase();
    name.starts_with("claude-")
        && !name.ends_with('-')
        && !name.contains("--")
        && !name.contains("..")
        && !name.ends_with("-latest")
        && !name.contains("-latest-")
        && !name.contains("-v1")
        && !name.contains("-v2")
        && !name.contains("-v3")
        && RECOGNIZED_FAMILIES.iter().any(|tag| name.contains(tag))
        && !DISQUALIFYING_KEYWORDS.iter().any(|kw| name.contains(kw))
}

const CLAUDE_DISPLAY_MARKERS: &[&str] = &["Haiku", "Sonnet", "Opus", "Claude "];

fn looks_like_claude_model_label(raw: &str) -> bool {
    let s = raw.trim();
    !s.is_empty()
        && !s.starts_with("claude-")
        && CLAUDE_DISPLAY_MARKERS.iter().any(|m| s.contains(m))
}

fn parse_claude_family_and_version(model: &str) -> Option<(&'static str, Vec<u32>)> {
    let normalized = normalize_claude_model_token(model)?;
    let segments: Vec<&str> = normalized.split('-').collect();
    if segments.first().copied() != Some("claude") || segments.len() < 3 {
        return None;
    }

    for family in ["haiku", "sonnet", "opus"] {
        if segments.get(1).copied() == Some(family) {
            return parse_version_segments(&segments[2..]).map(|ver| (family, ver));
        }
        if segments.last().copied() == Some(family) {
            return parse_version_segments(&segments[1..segments.len() - 1])
                .map(|ver| (family, ver));
        }
    }

    None
}

fn parse_version_segments(tokens: &[&str]) -> Option<Vec<u32>> {
    if tokens.is_empty() {
        return None;
    }
    let mut digits = Vec::with_capacity(tokens.len());
    for seg in tokens {
        // Accept 1-2 digit numeric segments only (0..=99).
        if seg.is_empty() || seg.len() > 2 || !seg.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        digits.push(seg.parse::<u32>().ok()?);
    }
    Some(digits)
}
