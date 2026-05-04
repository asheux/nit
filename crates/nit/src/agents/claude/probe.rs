use std::fs;

use crate::agents::discover::{
    capture_cli_help_text, find_executable_in_path, probe_models_from_cli,
    DEFAULT_MODEL_LIST_ARG_SETS,
};

use super::binary::{parse_claude_models_from_binary, select_current_claude_models};

pub(super) const STANDARD_CONTEXT_WINDOW: u32 = 200_000;
pub(super) const EXTENDED_CONTEXT_WINDOW: u32 = 1_000_000;
pub(super) const MIN_ASCII_RUN_LENGTH: usize = 8;

const PREFERRED_DEFAULT_EFFORT: &str = "high";
const DEFAULT_CLAUDE_EFFORTS: &[&str] = &["low", "medium", "high"];

type ModelProbeResult = (Vec<String>, Option<String>);

pub(in crate::agents) fn probe_claude_models() -> ModelProbeResult {
    let (cli_raw_output, cli_error) = probe_models_from_cli("claude", DEFAULT_MODEL_LIST_ARG_SETS);

    let cli_filtered = select_current_claude_models(cli_raw_output);
    if !cli_filtered.is_empty() {
        return (cli_filtered, None);
    }

    let binary_models = find_executable_in_path("claude")
        .and_then(|exe| fs::read(exe).ok())
        .map(|bytes| parse_claude_models_from_binary(&bytes))
        .filter(|models| !models.is_empty());
    if let Some(candidates) = binary_models {
        return (select_current_claude_models(candidates), None);
    }

    (cli_filtered, cli_error)
}

pub(super) fn probe_claude_supported_efforts() -> Vec<String> {
    capture_cli_help_text("claude")
        .as_deref()
        .and_then(parse_effort_choices_from_help)
        .unwrap_or_else(|| DEFAULT_CLAUDE_EFFORTS.iter().map(|s| (*s).into()).collect())
}

pub(super) fn pick_claude_default_effort(supported: &[String]) -> String {
    [PREFERRED_DEFAULT_EFFORT, "medium", "low"]
        .iter()
        .find_map(|target| {
            supported
                .iter()
                .find(|effort| effort.eq_ignore_ascii_case(target))
                .cloned()
        })
        .or_else(|| supported.first().cloned())
        .unwrap_or_else(|| PREFERRED_DEFAULT_EFFORT.to_string())
}

pub(crate) fn parse_effort_choices_from_help(help_output: &str) -> Option<Vec<String>> {
    let needle = "--effort";
    let start = help_output.find(needle)?;
    let after = &help_output[start + needle.len()..];
    let open = after.find('(')?;
    let close = after[open + 1..].find(')')?;
    let raw = &after[open + 1..open + 1 + close];

    let mut choices: Vec<String> = raw
        .split(',')
        .map(|piece| piece.trim().to_ascii_lowercase())
        .filter(|piece| !piece.is_empty() && piece.chars().all(|c| c.is_ascii_alphanumeric()))
        .collect();

    let rank = |effort: &str| match effort {
        "low" => 0u8,
        "medium" => 1,
        "high" => 2,
        "xhigh" => 3,
        "max" => 4,
        _ => 10,
    };
    choices.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
    choices.dedup();

    (!choices.is_empty()).then_some(choices)
}
