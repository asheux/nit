mod parse;
mod path;
mod runner;

pub(in crate::agents) use path::find_executable_in_path;
pub(in crate::agents) use runner::{capture_cli_help_text, probe_models_from_cli};

pub(in crate::agents) fn codex_cli_available() -> bool {
    is_executable_in_path("codex")
}

pub(in crate::agents) fn claude_cli_available() -> bool {
    is_executable_in_path("claude")
}

pub(in crate::agents) fn gemini_cli_available() -> bool {
    is_executable_in_path("gemini")
}

fn is_executable_in_path(binary_name: &str) -> bool {
    find_executable_in_path(binary_name).is_some()
}
