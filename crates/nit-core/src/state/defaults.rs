//! Serde `#[serde(default = "...")]` callback helpers.
//!
//! Each entry must remain a distinct named fn-pointer because
//! `default = "..."` resolves a path, not a value — merging them or swapping
//! to a const would break deserialization of older snapshots.

use super::CONSOLE_SCROLL_BOTTOM;

const DEFAULT_PARALLEL_TURNS: usize = 2;
const DEFAULT_SWARM_TEMPLATE: &str = "lab";
const DEFAULT_SWARM_MISSION: &str = "auto";

// `usize::MAX` sentinels meaning "no render yet, scroll is unclamped".
pub(super) fn scroll_cache_sentinel() -> usize {
    usize::MAX
}
pub(super) fn gate_monitor_max_scroll_default() -> usize {
    usize::MAX
}
pub(super) fn artifacts_popup_last_max_scroll_default() -> usize {
    usize::MAX
}

// Console-style scrollers default to "auto-scroll to bottom".
pub(super) fn chat_input_scroll_default() -> usize {
    CONSOLE_SCROLL_BOTTOM
}
pub(super) fn chat_thread_scroll_default() -> usize {
    CONSOLE_SCROLL_BOTTOM
}

// Swarm template / mission defaults — kept as paired pairs so older
// snapshots written by either name continue to deserialize cleanly.
pub(super) fn swarm_default_template_default() -> String {
    DEFAULT_SWARM_TEMPLATE.into()
}
pub(super) fn swarm_default_mission_default() -> String {
    DEFAULT_SWARM_MISSION.into()
}
pub(super) fn default_swarm_template() -> String {
    DEFAULT_SWARM_TEMPLATE.into()
}
pub(super) fn default_swarm_mission() -> String {
    DEFAULT_SWARM_MISSION.into()
}

pub(super) fn codex_max_parallel_turns_default() -> usize {
    DEFAULT_PARALLEL_TURNS
}
pub(super) fn claude_max_parallel_turns_default() -> usize {
    DEFAULT_PARALLEL_TURNS
}
