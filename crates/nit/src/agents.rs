use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::Deserialize;

use crate::cli::AgentsArg;

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

fn load_agents_from_codex_models_cache() -> anyhow::Result<nit_core::AgentsState> {
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

pub(crate) fn load_only_codex_agents() -> nit_core::AgentsState {
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

fn claude_lane() -> nit_core::AgentLane {
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

pub(crate) fn load_only_claude_agents() -> nit_core::AgentsState {
    let mut agents = nit_core::AgentsState::default();
    if !claude_cli_available() {
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

pub(crate) fn load_local_agent_lane() -> nit_core::AgentsState {
    let mut agents = nit_core::AgentsState::default();
    agents.agents.push(nit_core::AgentLane {
        id: "local".into(),
        role: "Local".into(),
        lane: "Local".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: "Built-in local lane.".into(),
    });
    agents.selected_agent = Some("local".into());
    agents.roster_selected = 0;
    agents
}

pub(crate) fn load_all_available_agents() -> nit_core::AgentsState {
    let mut agents = nit_core::AgentsState::default();
    agents.agents.extend(load_local_agent_lane().agents);

    if claude_cli_available() {
        agents.agents.push(claude_lane());
    }

    if codex_cli_available() {
        match load_agents_from_codex_models_cache() {
            Ok(codex_agents) => {
                agents.codex_effective_context_window_tokens =
                    codex_agents.codex_effective_context_window_tokens;
                agents.codex_default_reasoning_effort = codex_agents.codex_default_reasoning_effort;
                agents.codex_supported_reasoning_efforts =
                    codex_agents.codex_supported_reasoning_efforts;
                agents.codex_selected_reasoning_effort =
                    codex_agents.codex_selected_reasoning_effort;
                agents.agents.extend(codex_agents.agents);
                agents.mcp = codex_agents.mcp;
            }
            Err(err) => {
                agents.alerts.push(nit_core::AgentAlert {
                    severity: nit_core::AgentAlertSeverity::Warn,
                    source: "codex".into(),
                    message: format!("Failed to load Codex models: {err}"),
                    at: "t+0".into(),
                });
            }
        }
    }

    agents.selected_agent = agents.agents.first().map(|a| a.id.clone());
    agents.roster_selected = 0;
    agents
}

pub(crate) fn init_agents(agents_arg: AgentsArg) -> nit_core::AgentsState {
    let mut agents = match agents_arg {
        AgentsArg::Local => load_local_agent_lane(),
        AgentsArg::Codex => load_only_codex_agents(),
        AgentsArg::Claude => load_only_claude_agents(),
        AgentsArg::All => load_all_available_agents(),
    };
    agents.codex_cli_available = codex_cli_available();
    agents.claude_cli_available = claude_cli_available();
    agents.gemini_cli_available = gemini_cli_available();
    if matches!(agents_arg, AgentsArg::All | AgentsArg::Claude) && agents.claude_cli_available {
        let (models, error) = probe_claude_models();
        agents.claude_models = models;
        agents.claude_models_error = error;
        populate_claude_model_metadata(&mut agents);
    } else {
        agents.claude_models.clear();
        agents.claude_models_error = None;
    }
    if matches!(agents_arg, AgentsArg::All) && agents.gemini_cli_available {
        let (models, error) = probe_gemini_models();
        agents.gemini_models = models;
        agents.gemini_models_error = error;
    } else {
        agents.gemini_models.clear();
        agents.gemini_models_error = None;
    }
    sync_backend_model_lanes(&mut agents, agents_arg);
    agents
}

pub(crate) fn codex_cli_available() -> bool {
    is_executable_in_path("codex")
}

pub(crate) fn claude_cli_available() -> bool {
    is_executable_in_path("claude")
}

pub(crate) fn gemini_cli_available() -> bool {
    is_executable_in_path("gemini")
}

pub(crate) fn probe_claude_models() -> (Vec<String>, Option<String>) {
    let (models, error) = probe_models_from_cli(
        "claude",
        &[
            &["models", "--json"],
            &["models"],
            &["list-models"],
            &["--list-models"],
        ],
    );
    let models = select_current_claude_models(models);
    if !models.is_empty() {
        return (models, None);
    }

    if let Some(models) = probe_claude_models_from_install() {
        let models = select_current_claude_models(models);
        return (models, None);
    }

    (models, error)
}

pub(crate) fn probe_gemini_models() -> (Vec<String>, Option<String>) {
    let (models, error) = probe_models_from_cli(
        "gemini",
        &[
            &["models", "--json"],
            &["models"],
            &["list-models"],
            &["--list-models"],
            &["--models"],
        ],
    );
    let models = select_current_gemini_models(models);
    if !models.is_empty() {
        return (models, None);
    }

    if let Some(models) = probe_gemini_models_from_install() {
        let models = select_current_gemini_models(models);
        return (models, None);
    }

    (models, error)
}

fn probe_models_from_cli(bin: &str, attempts: &[&[&str]]) -> (Vec<String>, Option<String>) {
    let timeout = Duration::from_millis(1500);
    let mut last_err: Option<String> = None;

    for args in attempts {
        match run_command_capture_timeout(bin, args, timeout) {
            Ok((status, stdout, stderr)) => {
                if !status.success() {
                    let err = String::from_utf8_lossy(&stderr).trim().to_string();
                    last_err = Some(if err.is_empty() {
                        format!("{bin} {} exited with {status}", args.join(" "))
                    } else {
                        err
                    });
                    continue;
                }

                let models = parse_model_list_from_output(&stdout);
                if !models.is_empty() {
                    return (models, None);
                }

                let err = String::from_utf8_lossy(&stderr).trim().to_string();
                last_err = Some(if err.is_empty() {
                    format!("{bin} {} returned no models", args.join(" "))
                } else {
                    err
                });
            }
            Err(err) => {
                last_err = Some(err.to_string());
            }
        }
    }

    (Vec::new(), last_err)
}

pub(crate) fn sync_backend_model_lanes(agents: &mut nit_core::AgentsState, agents_arg: AgentsArg) {
    let wants_claude = matches!(agents_arg, AgentsArg::All | AgentsArg::Claude);
    let wants_gemini = matches!(agents_arg, AgentsArg::All);
    let replace_claude = wants_claude && !agents.claude_models.is_empty();
    let replace_gemini = wants_gemini && !agents.gemini_models.is_empty();

    if !replace_claude && !replace_gemini {
        return;
    }

    let selected_agent = agents.selected_agent.clone();
    let mut updated: Vec<nit_core::AgentLane> = Vec::with_capacity(
        agents.agents.len()
            + agents
                .claude_models
                .len()
                .saturating_add(agents.gemini_models.len()),
    );

    for lane in agents.agents.drain(..) {
        if replace_claude && matches!(lane.kind, nit_core::AgentLaneKind::Claude) {
            continue;
        }
        if replace_gemini && matches!(lane.kind, nit_core::AgentLaneKind::Gemini) {
            continue;
        }
        updated.push(lane);
    }

    if replace_claude {
        for model in agents.claude_models.iter() {
            updated.push(nit_core::AgentLane {
                id: model.clone(),
                role: model.clone(),
                lane: "Claude".into(),
                kind: nit_core::AgentLaneKind::Claude,
                status: nit_core::AgentStatus::Idle,
                heartbeat_age_secs: 0,
                queue_len: 0,
                current_mission: None,
                last_message: String::new(),
            });
        }
    }

    if replace_gemini {
        for model in agents.gemini_models.iter() {
            updated.push(nit_core::AgentLane {
                id: model.clone(),
                role: model.clone(),
                lane: "Gemini".into(),
                kind: nit_core::AgentLaneKind::Gemini,
                status: nit_core::AgentStatus::Idle,
                heartbeat_age_secs: 0,
                queue_len: 0,
                current_mission: None,
                last_message: String::new(),
            });
        }
    }

    agents.agents = updated;

    if let Some(selected) = selected_agent {
        if let Some(idx) = agents.agents.iter().position(|lane| lane.id == selected) {
            agents.selected_agent = Some(selected);
            agents.roster_selected = idx;
            return;
        }
    }

    agents.selected_agent = agents.agents.first().map(|lane| lane.id.clone());
    agents.roster_selected = 0;
}

/// Populate Claude model metadata (context windows, effort levels) for all probed models.
pub(crate) fn populate_claude_model_metadata(agents: &mut nit_core::AgentsState) {
    for model in agents.claude_models.iter() {
        // Determine context window based on model name.
        // Models with "[1m]" suffix have 1M context; others default to 200k.
        let context_window: u32 = if model.contains("[1m]") || model.contains("1m") {
            1_000_000
        } else {
            200_000
        };
        agents
            .claude_effective_context_window_tokens
            .insert(model.clone(), context_window);

        // Determine supported effort levels. "max" is only available on Opus models.
        let is_opus = model.to_lowercase().contains("opus");
        let supported = if is_opus {
            vec![
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "max".to_string(),
            ]
        } else {
            vec!["low".to_string(), "medium".to_string(), "high".to_string()]
        };
        agents
            .claude_supported_efforts
            .insert(model.clone(), supported);

        // Default effort: "high" for all Claude models.
        agents
            .claude_default_effort
            .insert(model.clone(), "high".to_string());
        // Initialize selected effort to default.
        agents
            .claude_selected_effort
            .insert(model.clone(), "high".to_string());
    }
}

fn run_command_capture_timeout(
    bin: &str,
    args: &[&str],
    timeout: Duration,
) -> io::Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)> {
    let executable = find_executable_in_path(bin).unwrap_or_else(|| PathBuf::from(bin));
    let mut command = ProcessCommand::new(&executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path_override) = preferred_path_for_executable(&executable) {
        command.env("PATH", path_override);
    }
    let mut child = command.spawn()?;

    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            if let Some(mut out) = child.stdout.take() {
                let _ = out.read_to_end(&mut stdout);
            }
            if let Some(mut err) = child.stderr.take() {
                let _ = err.read_to_end(&mut stderr);
            }
            return Ok((status, stdout, stderr));
        }

        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{bin} {} timed out after {timeout:?}", args.join(" ")),
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn probe_gemini_models_from_install() -> Option<Vec<String>> {
    let executable = find_executable_in_path("gemini")?;
    let resolved = fs::canonicalize(executable).ok()?;
    let package_root = resolved.parent()?.parent()?;
    let models_js = package_root
        .join("node_modules")
        .join("@google")
        .join("gemini-cli-core")
        .join("dist")
        .join("src")
        .join("config")
        .join("models.js");
    let source = fs::read_to_string(models_js).ok()?;
    let models = parse_gemini_models_from_source(&source);
    if models.is_empty() {
        None
    } else {
        Some(models)
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

pub(crate) fn parse_claude_models_from_binary(bytes: &[u8]) -> Vec<String> {
    let ascii_runs = extract_ascii_runs(bytes);

    let mut models = Vec::new();
    for pair in ascii_runs.windows(2) {
        let Some(candidate) = normalize_claude_model_token(&pair[0]) else {
            continue;
        };
        if looks_like_claude_model_label(&pair[1]) {
            models.push(candidate.to_string());
        }
    }
    models.sort();
    models.dedup();
    models
}

fn extract_ascii_runs(bytes: &[u8]) -> Vec<String> {
    let mut ascii_runs = Vec::new();
    let mut start = None;

    for (idx, &byte) in bytes.iter().enumerate() {
        if byte.is_ascii_graphic() || byte == b' ' {
            if start.is_none() {
                start = Some(idx);
            }
            continue;
        }
        if let Some(run_start) = start.take() {
            if idx.saturating_sub(run_start) >= 8 {
                ascii_runs.push(String::from_utf8_lossy(&bytes[run_start..idx]).into_owned());
            }
        }
    }
    if let Some(run_start) = start {
        if bytes.len().saturating_sub(run_start) >= 8 {
            ascii_runs.push(String::from_utf8_lossy(&bytes[run_start..]).into_owned());
        }
    }

    ascii_runs
}

pub(crate) fn parse_gemini_models_from_source(source: &str) -> Vec<String> {
    let mut named_values = HashMap::new();
    for line in source.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("export const ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim().trim_end_matches(';').trim();
        if let Some(value) = parse_single_quoted_literal(value) {
            named_values.insert(name.to_string(), value.to_string());
        }
    }

    let marker = "export const VALID_GEMINI_MODELS = new Set([";
    let Some(start) = source.find(marker) else {
        return Vec::new();
    };
    let remainder = &source[start + marker.len()..];
    let Some(end) = remainder.find("]);") else {
        return Vec::new();
    };
    let mut models = Vec::new();
    for entry in remainder[..end].split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if let Some(value) = parse_single_quoted_literal(entry) {
            models.push(value.to_string());
            continue;
        }
        if let Some(value) = named_values.get(entry) {
            models.push(value.clone());
        }
    }
    models.sort();
    models.dedup();
    models
}

pub(crate) fn select_current_claude_models(models: Vec<String>) -> Vec<String> {
    let mut original = models;
    original.sort();
    original.dedup();

    let mut latest_by_family: HashMap<&'static str, (Vec<u32>, String)> = HashMap::new();
    for model in original.iter() {
        let Some((family, version)) = parse_claude_family_and_version(model) else {
            continue;
        };
        match latest_by_family.entry(family) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert((version, model.clone()));
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let (current_version, current_model) = entry.get();
                if version > *current_version
                    || (version == *current_version
                        && prefer_shorter_model_name(model, current_model))
                {
                    entry.insert((version, model.clone()));
                }
            }
        }
    }

    if latest_by_family.is_empty() {
        return original;
    }

    let mut current: Vec<String> = latest_by_family
        .into_values()
        .map(|(_version, model)| model)
        .collect();
    current.sort();
    current
}

pub(crate) fn select_current_gemini_models(models: Vec<String>) -> Vec<String> {
    let mut original = models;
    original.sort();
    original.dedup();

    let mut latest_by_family: HashMap<&'static str, (bool, Vec<u32>, String)> = HashMap::new();
    for model in original.iter() {
        let Some((family, preview, version)) = parse_gemini_family_preview_and_version(model)
        else {
            continue;
        };
        match latest_by_family.entry(family) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert((preview, version, model.clone()));
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let (current_preview, current_version, current_model) = entry.get();
                let better = (*current_preview && !preview)
                    || (*current_preview == preview
                        && (version > *current_version
                            || (version == *current_version
                                && prefer_shorter_model_name(model, current_model))));
                if better {
                    entry.insert((preview, version, model.clone()));
                }
            }
        }
    }

    if latest_by_family.is_empty() {
        return original;
    }

    let mut current: Vec<String> = latest_by_family
        .into_values()
        .map(|(_preview, _version, model)| model)
        .collect();
    current.sort();
    current
}

fn normalize_claude_model_token(raw: &str) -> Option<&str> {
    let candidate = raw.trim().strip_suffix("[1m]").unwrap_or(raw.trim());
    if is_probable_claude_model(candidate) {
        Some(candidate)
    } else {
        None
    }
}

fn is_probable_claude_model(candidate: &str) -> bool {
    let candidate = candidate.to_ascii_lowercase();
    if !candidate.starts_with("claude-")
        || candidate.ends_with('-')
        || candidate.contains("--")
        || candidate.contains("..")
        || candidate.ends_with("-latest")
        || candidate.contains("-latest-")
        || candidate.contains("-v1")
        || candidate.contains("-v2")
        || candidate.contains("-v3")
    {
        return false;
    }

    if !(candidate.contains("-haiku")
        || candidate.contains("-sonnet")
        || candidate.contains("-opus"))
    {
        return false;
    }

    ![
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
    ]
    .iter()
    .any(|needle| candidate.contains(needle))
}

fn looks_like_claude_model_label(label: &str) -> bool {
    let label = label.trim();
    !label.is_empty()
        && !label.starts_with("claude-")
        && (label.contains("Haiku")
            || label.contains("Sonnet")
            || label.contains("Opus")
            || label.contains("Claude "))
}

fn parse_claude_family_and_version(model: &str) -> Option<(&'static str, Vec<u32>)> {
    let candidate = normalize_claude_model_token(model)?;
    let parts: Vec<&str> = candidate.split('-').collect();
    if parts.first().copied() != Some("claude") || parts.len() < 3 {
        return None;
    }

    for family in ["haiku", "sonnet", "opus"] {
        if parts.get(1).copied() == Some(family) {
            return parse_small_numeric_parts(&parts[2..]).map(|version| (family, version));
        }
        if parts.last().copied() == Some(family) {
            return parse_small_numeric_parts(&parts[1..parts.len().saturating_sub(1)])
                .map(|version| (family, version));
        }
    }

    None
}

fn parse_gemini_family_preview_and_version(model: &str) -> Option<(&'static str, bool, Vec<u32>)> {
    let candidate = model.trim().to_ascii_lowercase();
    let rest = candidate.strip_prefix("gemini-")?;
    let (version, suffix) = rest.split_once('-')?;
    let version = parse_dot_numeric_parts(version)?;
    if suffix.contains("customtools") || suffix.contains("embedding") {
        return None;
    }

    let family = if suffix.contains("flash-lite") {
        "flash-lite"
    } else if suffix.contains("flash") {
        "flash"
    } else if suffix.contains("pro") {
        "pro"
    } else {
        return None;
    };

    Some((family, suffix.contains("preview"), version))
}

fn parse_small_numeric_parts(parts: &[&str]) -> Option<Vec<u32>> {
    if parts.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        if part.is_empty() || part.len() > 2 || !part.chars().all(|ch| ch.is_ascii_digit()) {
            return None;
        }
        let value = part.parse::<u32>().ok()?;
        if value > 99 {
            return None;
        }
        out.push(value);
    }
    Some(out)
}

fn parse_dot_numeric_parts(raw: &str) -> Option<Vec<u32>> {
    if raw.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for part in raw.split('.') {
        if part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()) {
            return None;
        }
        out.push(part.parse::<u32>().ok()?);
    }
    Some(out)
}

fn prefer_shorter_model_name(candidate: &str, current: &str) -> bool {
    candidate.len() < current.len() || (candidate.len() == current.len() && candidate < current)
}

fn parse_single_quoted_literal(value: &str) -> Option<&str> {
    let value = value.trim();
    let value = value.strip_prefix('\'')?;
    let value = value.strip_suffix('\'')?;
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn parse_model_list_from_output(stdout: &[u8]) -> Vec<String> {
    let raw = String::from_utf8_lossy(stdout);
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
        let mut out = Vec::new();
        extract_models_from_json(&value, &mut out);
        out.sort();
        out.dedup();
        return out;
    }

    let mut out = Vec::new();
    for line in raw.lines() {
        let mut line = line.trim();
        if line.is_empty() {
            continue;
        }
        line = line
            .trim_start_matches('-')
            .trim_start_matches('*')
            .trim_start_matches('•')
            .trim();
        if line.is_empty() {
            continue;
        }
        let Some(candidate) = line.split_whitespace().next() else {
            continue;
        };
        if candidate.ends_with(':') {
            continue;
        }
        if candidate.eq_ignore_ascii_case("models") || candidate.eq_ignore_ascii_case("model") {
            continue;
        }
        if candidate.len() < 3 {
            continue;
        }
        out.push(candidate.to_string());
    }
    out.sort();
    out.dedup();
    out
}

fn extract_models_from_json(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => {
            let s = s.trim();
            if !s.is_empty() {
                out.push(s.to_string());
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                extract_models_from_json(v, out);
            }
        }
        serde_json::Value::Object(map) => {
            for key in ["id", "name", "model", "slug"] {
                if let Some(serde_json::Value::String(s)) = map.get(key) {
                    let s = s.trim();
                    if !s.is_empty() {
                        out.push(s.to_string());
                        return;
                    }
                }
            }
            for key in ["models", "data"] {
                if let Some(v) = map.get(key) {
                    extract_models_from_json(v, out);
                }
            }
        }
        _ => {}
    }
}

fn is_executable_in_path(bin: &str) -> bool {
    find_executable_in_path(bin).is_some()
}

fn find_executable_in_path(bin: &str) -> Option<PathBuf> {
    for dir in executable_search_dirs() {
        if dir.as_os_str().is_empty() {
            continue;
        }
        #[cfg(windows)]
        {
            let mut exts = std::env::var_os("PATHEXT")
                .map(|v| {
                    v.to_string_lossy()
                        .split(';')
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.trim_start_matches('.').to_ascii_lowercase())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| vec!["exe".into(), "cmd".into(), "bat".into()]);
            if exts.is_empty() {
                exts = vec!["exe".into(), "cmd".into(), "bat".into()];
            }

            let candidate = dir.join(bin);
            if candidate.is_file() {
                return Some(candidate);
            }
            for ext in exts.iter() {
                let candidate = dir.join(format!("{bin}.{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        #[cfg(not(windows))]
        {
            let candidate = dir.join(bin);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn executable_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join("bin"));
    }

    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/opt/homebrew/sbin"));
    }

    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/usr/local/sbin"));

    let mut unique = Vec::new();
    for dir in dirs {
        if dir.as_os_str().is_empty() || unique.iter().any(|existing| existing == &dir) {
            continue;
        }
        unique.push(dir);
    }
    unique
}

fn preferred_path_for_executable(executable: &Path) -> Option<OsString> {
    let mut paths = Vec::<PathBuf>::new();
    if let Some(dir) = executable.parent() {
        paths.push(dir.to_path_buf());
    }
    paths.extend(executable_search_dirs());
    let mut deduped = Vec::new();
    for path in paths {
        if deduped.iter().any(|existing| existing == &path) {
            continue;
        }
        deduped.push(path);
    }
    std::env::join_paths(deduped).ok()
}
