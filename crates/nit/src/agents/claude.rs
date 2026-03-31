//! Claude backend model discovery, probing, and metadata population.
//!
//! Discovers Claude CLI models via command-line probing and binary inspection,
//! filters to the latest version per family, and populates context windows
//! and effort level metadata.

use std::collections::HashMap;
use std::fs;

use super::discover::{find_executable_in_path, probe_models_from_cli};

/// Default context window for most Claude models (tokens).
const STANDARD_CONTEXT_WINDOW: u32 = 200_000;

/// Extended context window for models with the 1M token option.
const EXTENDED_CONTEXT_WINDOW: u32 = 1_000_000;

/// Minimum length of an ASCII run to consider as a potential embedded string.
const MIN_ASCII_RUN_LENGTH: usize = 8;

/// Result of a model discovery attempt: list of model identifiers and an optional error message.
type ModelProbeResult = (Vec<String>, Option<String>);

/// Construct the default agent lane descriptor for the Claude backend.
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

/// Build an agent roster containing only the Claude backend lane.
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

/// Probe the Claude CLI and binary for available model identifiers.
///
/// First attempts CLI-based discovery using several argument variants.
/// Falls back to scanning the installed binary for embedded model strings.
pub(super) fn probe_claude_models() -> ModelProbeResult {
    // Phase 1: try several CLI invocations to enumerate models directly.
    let (cli_raw_output, cli_error) = probe_models_from_cli(
        "claude",
        &[
            &["models", "--json"],
            &["models"],
            &["list-models"],
            &["--list-models"],
        ],
    );

    // Deduplicate and select only the latest version per family.
    let cli_filtered = select_current_claude_models(cli_raw_output);
    if !cli_filtered.is_empty() {
        return (cli_filtered, None);
    }

    // Phase 2: fall back to reading the installed binary for embedded strings.
    if let Some(binary_candidates) = probe_claude_models_from_install() {
        let binary_filtered = select_current_claude_models(binary_candidates);
        return (binary_filtered, None);
    }

    // Neither approach yielded results; propagate the original CLI error.
    (cli_filtered, cli_error)
}

/// Populate Claude model metadata (context windows, effort levels) for all probed models.
pub(super) fn populate_claude_model_metadata(roster: &mut nit_core::AgentsState) {
    let model_identifiers: Vec<String> = roster.claude_models.clone();
    for model_id in &model_identifiers {
        insert_claude_context_window(roster, model_id);
        insert_claude_effort_levels(roster, model_id);
    }
}

/// Determine and record the effective context window for a Claude model identifier.
fn insert_claude_context_window(roster: &mut nit_core::AgentsState, model_id: &str) {
    let effective_window_size: u32 =
        if model_id.contains("[1m]") || model_id.contains("1m") {
            EXTENDED_CONTEXT_WINDOW
        } else {
            STANDARD_CONTEXT_WINDOW
        };
    roster
        .claude_effective_context_window_tokens
        .insert(model_id.to_owned(), effective_window_size);
}

/// Record the supported, default, and selected effort levels for a Claude model.
fn insert_claude_effort_levels(roster: &mut nit_core::AgentsState, model_id: &str) {
    let owned_key = model_id.to_owned();
    let supports_max_effort = model_id.to_lowercase().contains("opus");
    let effort_tiers = if supports_max_effort {
        vec!["low".into(), "medium".into(), "high".into(), "max".into()]
    } else {
        vec!["low".into(), "medium".into(), "high".into()]
    };
    roster
        .claude_supported_efforts
        .insert(owned_key.clone(), effort_tiers);
    roster
        .claude_default_effort
        .insert(owned_key.clone(), "high".into());
    roster
        .claude_selected_effort
        .insert(owned_key, "high".into());
}

/// Extract model identifiers from a Claude binary's embedded string table.
pub(crate) fn parse_claude_models_from_binary(bytes: &[u8]) -> Vec<String> {
    let collected_fragments = extract_ascii_runs(bytes);

    let mut discovered_models = Vec::new();
    for adjacent_tokens in collected_fragments.windows(2) {
        let Some(validated_model) = normalize_claude_model_token(&adjacent_tokens[0]) else {
            continue;
        };
        if looks_like_claude_model_label(&adjacent_tokens[1]) {
            discovered_models.push(validated_model.to_string());
        }
    }
    discovered_models.sort();
    discovered_models.dedup();
    discovered_models
}

/// Deduplicate and keep only the latest version per model family.
///
/// Sorts and deduplicates the input, then groups candidates by family tag
/// and retains only the highest-versioned entry for each family.
pub(crate) fn select_current_claude_models(models: Vec<String>) -> Vec<String> {
    // Sort and deduplicate the raw input list.
    let mut deduplicated_input = models;
    deduplicated_input.sort();
    deduplicated_input.dedup();

    // Accumulate the best (latest version) candidate per family.
    let mut latest_per_family: HashMap<&'static str, (Vec<u32>, String)> = HashMap::new();
    for candidate_model in deduplicated_input.iter() {
        let Some((family_tag, parsed_version)) =
            parse_claude_family_and_version(candidate_model)
        else {
            continue;
        };
        update_latest_per_family(
            &mut latest_per_family,
            family_tag,
            parsed_version,
            candidate_model,
        );
    }

    // If no families were recognized, return the full deduplicated list.
    if latest_per_family.is_empty() {
        return deduplicated_input;
    }

    // Collect the winning model from each family, sorted for determinism.
    let mut result: Vec<String> = latest_per_family
        .into_values()
        .map(|(_version, model)| model)
        .collect();
    result.sort();
    result
}

/// Insert or update the latest model version for a given family tag.
fn update_latest_per_family(
    map: &mut HashMap<&'static str, (Vec<u32>, String)>,
    family_tag: &'static str,
    parsed_version: Vec<u32>,
    candidate_model: &str,
) {
    match map.entry(family_tag) {
        std::collections::hash_map::Entry::Vacant(slot) => {
            slot.insert((parsed_version, candidate_model.to_owned()));
        }
        std::collections::hash_map::Entry::Occupied(mut slot) => {
            let dominated = is_version_dominated(
                &parsed_version,
                candidate_model,
                slot.get(),
            );
            if dominated {
                slot.insert((parsed_version, candidate_model.to_owned()));
            }
        }
    }
}

/// Determine if the candidate version should replace the current best.
fn is_version_dominated(
    candidate_ver: &[u32],
    candidate_name: &str,
    (incumbent_ver, incumbent_name): &(Vec<u32>, String),
) -> bool {
    candidate_ver > incumbent_ver.as_slice()
        || (candidate_ver == incumbent_ver.as_slice()
            && super::prefer_shorter_model_name(candidate_name, incumbent_name))
}

/// Attempt to discover Claude models by reading the installed binary directly.
fn probe_claude_models_from_install() -> Option<Vec<String>> {
    let executable = find_executable_in_path("claude")?;
    let bytes = fs::read(executable).ok()?;
    let discovered_models = parse_claude_models_from_binary(&bytes);
    if discovered_models.is_empty() {
        None
    } else {
        Some(discovered_models)
    }
}

/// Scan raw bytes for contiguous ASCII runs of at least [`MIN_ASCII_RUN_LENGTH`] characters.
///
/// Walks the byte slice linearly, accumulating runs of printable ASCII
/// (graphic characters and space). When a non-printable byte is encountered,
/// any run meeting the minimum length threshold is captured.
fn extract_ascii_runs(bytes: &[u8]) -> Vec<String> {
    if bytes.is_empty() {
        return Vec::new();
    }

    let mut collected_fragments = Vec::new();
    let mut run_begin: Option<usize> = None;

    // Walk each byte, tracking the start of the current ASCII run.
    for (byte_offset, &octet) in bytes.iter().enumerate() {
        if octet.is_ascii_graphic() || octet == b' ' {
            // Continue extending the current run.
            if run_begin.is_none() {
                run_begin = Some(byte_offset);
            }
            continue;
        }
        // Non-printable byte: flush the current run if long enough.
        if let Some(confirmed_begin) = run_begin.take() {
            if byte_offset.saturating_sub(confirmed_begin) >= MIN_ASCII_RUN_LENGTH {
                collected_fragments.push(
                    String::from_utf8_lossy(&bytes[confirmed_begin..byte_offset]).into_owned(),
                );
            }
        }
    }

    // Flush any trailing run that extends to the end of the buffer.
    if let Some(confirmed_begin) = run_begin {
        if bytes.len().saturating_sub(confirmed_begin) >= MIN_ASCII_RUN_LENGTH {
            collected_fragments
                .push(String::from_utf8_lossy(&bytes[confirmed_begin..]).into_owned());
        }
    }

    collected_fragments
}

/// Strip a trailing context-window suffix and validate as a probable Claude model token.
fn normalize_claude_model_token(raw: &str) -> Option<&str> {
    let stripped_token = raw.trim().strip_suffix("[1m]").unwrap_or(raw.trim());
    if is_probable_claude_model(stripped_token) {
        Some(stripped_token)
    } else {
        None
    }
}

/// Return true when the raw candidate looks like a well-formed Claude model identifier.
fn is_probable_claude_model(raw_candidate: &str) -> bool {
    let lowered_name = raw_candidate.to_ascii_lowercase();
    has_valid_claude_prefix(&lowered_name)
        && contains_model_family_marker(&lowered_name)
        && !contains_disqualifying_keyword(&lowered_name)
}

/// Check that the identifier has the `claude-` prefix and no malformed separators or suffixes.
fn has_valid_claude_prefix(lowered_name: &str) -> bool {
    let has_required_prefix = lowered_name.starts_with("claude-");
    let has_clean_separators = !lowered_name.ends_with('-')
        && !lowered_name.contains("--")
        && !lowered_name.contains("..");
    let lacks_version_suffixes = !lowered_name.ends_with("-latest")
        && !lowered_name.contains("-latest-")
        && !lowered_name.contains("-v1")
        && !lowered_name.contains("-v2")
        && !lowered_name.contains("-v3");
    has_required_prefix && has_clean_separators && lacks_version_suffixes
}

/// Suffixes that identify recognized model families in Claude identifiers.
const RECOGNIZED_FAMILIES: &[&str] = &["-haiku", "-sonnet", "-opus"];

/// Check that the identifier contains a recognized model family name.
fn contains_model_family_marker(normalized_id: &str) -> bool {
    RECOGNIZED_FAMILIES.iter().any(|tag| normalized_id.contains(tag))
}

/// Keywords that indicate an internal tool, plugin, or non-model artifact.
const DISQUALIFYING_MODEL_KEYWORDS: &[&str] = &[
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

/// Check whether the identifier contains any keyword that disqualifies it as a model.
fn contains_disqualifying_keyword(lowered_name: &str) -> bool {
    DISQUALIFYING_MODEL_KEYWORDS
        .iter()
        .any(|keyword| lowered_name.contains(keyword))
}

/// Well-known display name fragments that accompany Claude model tokens.
const CLAUDE_DISPLAY_MARKERS: &[&str] = &["Haiku", "Sonnet", "Opus", "Claude "];

/// Determine whether a string looks like a human-readable Claude model label.
fn looks_like_claude_model_label(raw_label: &str) -> bool {
    let trimmed_label = raw_label.trim();
    !trimmed_label.is_empty()
        && !trimmed_label.starts_with("claude-")
        && CLAUDE_DISPLAY_MARKERS
            .iter()
            .any(|marker| trimmed_label.contains(marker))
}

/// Parse a model identifier into its family name and numeric version segments.
fn parse_claude_family_and_version(model: &str) -> Option<(&'static str, Vec<u32>)> {
    let normalized_token = normalize_claude_model_token(model)?;
    let segments: Vec<&str> = normalized_token.split('-').collect();
    if segments.first().copied() != Some("claude") || segments.len() < 3 {
        return None;
    }

    for family_name in ["haiku", "sonnet", "opus"] {
        if segments.get(1).copied() == Some(family_name) {
            return parse_version_segments(&segments[2..]).map(|ver| (family_name, ver));
        }
        if segments.last().copied() == Some(family_name) {
            return parse_version_segments(&segments[1..segments.len().saturating_sub(1)])
                .map(|ver| (family_name, ver));
        }
    }

    None
}

/// Parse a slice of string segments into a vector of small numeric version digits.
fn parse_version_segments(version_tokens: &[&str]) -> Option<Vec<u32>> {
    if version_tokens.is_empty() {
        return None;
    }
    let mut parsed_digits = Vec::with_capacity(version_tokens.len());
    for segment in version_tokens {
        if segment.is_empty()
            || segment.len() > 2
            || !segment.chars().all(|ch| ch.is_ascii_digit())
        {
            return None;
        }
        let digit_value = segment.parse::<u32>().ok()?;
        if digit_value > 99 {
            return None;
        }
        parsed_digits.push(digit_value);
    }
    Some(parsed_digits)
}
