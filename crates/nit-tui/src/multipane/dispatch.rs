use nit_core::AppState;

use crate::app::dispatch_agent_prompt;
use crate::claude_runner::ClaudeRunner;
use crate::codex_runner::CodexRunner;
use crate::vitals::VitalsState;

/// Dispatch the focused-pane prompt through the standard
/// `dispatch_agent_prompt` path. The pane's `cwd` is read at the dispatch
/// leaf via `app::dispatch::resolve_dispatch_cwd`, so this wrapper has no
/// `cwd` parameter — queue dequeues stay correct because the leaf
/// resolves at dispatch time, not enqueue time.
pub(crate) fn dispatch_pane_prompt(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    pane_idx: usize,
    prompt: String,
) {
    let Some(mp) = state.multipane.as_ref() else {
        return;
    };
    let Some(pane) = mp.panes.get(pane_idx) else {
        return;
    };
    let agent_id = pane.agent_id.clone();
    let mission_id = pane.mission_id.clone();
    dispatch_agent_prompt(state, vitals, codex, claude, agent_id, mission_id, prompt);
}
