use std::fs;

use crate::agents::discover::{
    capture_cli_help_text, find_executable_in_path, probe_models_from_cli,
};

use super::binary::{parse_claude_models_from_binary, select_current_claude_models};
use super::effort::parse_effort_choices_from_help;

pub(super) const STANDARD_CONTEXT_WINDOW: u32 = 200_000;
pub(super) const EXTENDED_CONTEXT_WINDOW: u32 = 1_000_000;
pub(super) const MIN_ASCII_RUN_LENGTH: usize = 8;
pub(super) const PREFERRED_DEFAULT_EFFORT: &str = "high";

const DEFAULT_CLAUDE_EFFORTS: &[&str] = &["low", "medium", "high"];

type ModelProbeResult = (Vec<String>, Option<String>);

pub(in crate::agents) fn probe_claude_models() -> ModelProbeResult {
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

pub(super) fn probe_claude_supported_efforts() -> Vec<String> {
    capture_cli_help_text("claude")
        .as_deref()
        .and_then(parse_effort_choices_from_help)
        .unwrap_or_else(fallback_claude_efforts)
}

fn probe_claude_models_from_install() -> Option<Vec<String>> {
    let executable = find_executable_in_path("claude")?;
    let bytes = fs::read(executable).ok()?;
    let models = parse_claude_models_from_binary(&bytes);
    (!models.is_empty()).then_some(models)
}

fn fallback_claude_efforts() -> Vec<String> {
    DEFAULT_CLAUDE_EFFORTS.iter().map(|s| (*s).into()).collect()
}
