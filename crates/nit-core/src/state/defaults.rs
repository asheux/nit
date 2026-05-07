//! Serde `#[serde(default = "...")]` callback helpers.
//!
//! Each must remain a distinct named fn-pointer because `default = "..."`
//! resolves a path, not a value — these cannot be merged or replaced with a
//! const without breaking deserialization of older snapshots.

pub(super) fn scroll_cache_sentinel() -> usize {
    usize::MAX
}

pub(super) fn gate_monitor_max_scroll_default() -> usize {
    usize::MAX
}

pub(super) fn chat_input_scroll_default() -> usize {
    super::CONSOLE_SCROLL_BOTTOM
}

pub(super) fn artifacts_popup_last_max_scroll_default() -> usize {
    usize::MAX
}

pub(super) fn swarm_default_template_default() -> String {
    "lab".into()
}

pub(super) fn swarm_default_mission_default() -> String {
    "auto".into()
}

pub(super) fn codex_max_parallel_turns_default() -> usize {
    2
}

pub(super) fn claude_max_parallel_turns_default() -> usize {
    2
}

pub(super) fn chat_thread_scroll_default() -> usize {
    super::CONSOLE_SCROLL_BOTTOM
}

pub(super) fn default_swarm_template() -> String {
    "lab".into()
}

pub(super) fn default_swarm_mission() -> String {
    "auto".into()
}
