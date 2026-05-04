use std::collections::HashMap;

use crate::agents::prefer_shorter_model_name;

use super::probe::MIN_ASCII_RUN_LENGTH;

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

const CLAUDE_DISPLAY_MARKERS: &[&str] = &["Haiku", "Sonnet", "Opus", "Claude "];

const VERSION_DISQUALIFIERS: &[&str] = &["-latest", "-v1", "-v2", "-v3"];

pub(crate) fn parse_claude_models_from_binary(bytes: &[u8]) -> Vec<String> {
    let fragments = extract_ascii_runs(bytes);

    let mut models = Vec::new();
    for pair in fragments.windows(2) {
        let Some(model) = normalize_claude_model_token(&pair[0]) else {
            continue;
        };
        let label = pair[1].trim();
        let label_matches_display = !label.is_empty()
            && !label.starts_with("claude-")
            && CLAUDE_DISPLAY_MARKERS.iter().any(|m| label.contains(m));
        if label_matches_display {
            models.push(model.to_string());
        }
    }
    models.sort();
    models.dedup();
    models
}

pub(crate) fn select_current_claude_models(models: Vec<String>) -> Vec<String> {
    let mut deduped = models;
    deduped.sort();
    deduped.dedup();

    let mut best_per_family: HashMap<&'static str, (Vec<u32>, String)> = HashMap::new();
    for model in deduped.iter() {
        let Some((family, version)) = parse_claude_family_and_version(model) else {
            continue;
        };
        let dominated = best_per_family
            .get(family)
            .is_some_and(|(inc_ver, inc_name)| {
                version < *inc_ver
                    || (version == *inc_ver && !prefer_shorter_model_name(model, inc_name))
            });
        if !dominated {
            best_per_family.insert(family, (version, model.clone()));
        }
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

fn extract_ascii_runs(bytes: &[u8]) -> Vec<String> {
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;

    // The trailing 0u8 sentinel terminates any open run at index == bytes.len()
    // so the trailing-run case folds into the same branch as the in-loop case.
    for (i, &b) in bytes.iter().chain(std::iter::once(&0u8)).enumerate() {
        if b.is_ascii_graphic() || b == b' ' {
            start.get_or_insert(i);
            continue;
        }
        if let Some(begin) = start.take() {
            if i - begin >= MIN_ASCII_RUN_LENGTH {
                runs.push(String::from_utf8_lossy(&bytes[begin..i]).into_owned());
            }
        }
    }

    runs
}

fn normalize_claude_model_token(raw: &str) -> Option<&str> {
    let stripped = raw.trim().strip_suffix("[1m]").unwrap_or(raw.trim());
    is_probable_claude_model(stripped).then_some(stripped)
}

fn is_probable_claude_model(raw: &str) -> bool {
    let name = raw.to_ascii_lowercase();
    if !name.starts_with("claude-") || name.ends_with('-') {
        return false;
    }
    if name.contains("--") || name.contains("..") {
        return false;
    }
    if VERSION_DISQUALIFIERS.iter().any(|tag| name.contains(tag)) {
        return false;
    }
    RECOGNIZED_FAMILIES.iter().any(|tag| name.contains(tag))
        && !DISQUALIFYING_KEYWORDS.iter().any(|kw| name.contains(kw))
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
    tokens
        .iter()
        .map(|seg| {
            if seg.is_empty() || seg.len() > 2 || !seg.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            seg.parse::<u32>().ok()
        })
        .collect()
}
