//! App-loop integration tests. Sub-modules are organised by feature area:
//! agent_chat / chat / codex / dispatch / editor / genome_retry / helpers /
//! keymap / misc / mission / missions / mouse / multipane / popups / roster
//! / swarm. Shared helpers live here so each sub-module can scaffold an
//! `AppState` or fire a `KeyEvent` without duplicating the call-site.

use super::chat_input::push_chat_message;
use super::*;
use crate::swarm::{is_agent_busy, SwarmSize};
use crate::widgets::{agent_console_view, agent_ops_view};
use nit_core::AgentBusEvent;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

#[path = "tests/agent_chat.rs"]
mod agent_chat;
#[path = "tests/chat.rs"]
mod chat;
#[path = "tests/codex.rs"]
mod codex;
#[path = "tests/dispatch.rs"]
mod dispatch;
#[path = "tests/editor.rs"]
mod editor;
#[path = "tests/genome_retry.rs"]
mod genome_retry;
#[path = "tests/helpers.rs"]
mod helpers;
#[path = "tests/keymap.rs"]
mod keymap;
#[path = "tests/misc.rs"]
mod misc;
#[path = "tests/mission.rs"]
mod mission;
#[path = "tests/missions.rs"]
mod missions;
#[path = "tests/mouse.rs"]
mod mouse;
#[path = "tests/multipane.rs"]
mod multipane;
#[path = "tests/popups.rs"]
mod popups;
#[path = "tests/roster.rs"]
mod roster;
#[path = "tests/swarm.rs"]
mod swarm;

fn handle_agent_station_key(
    key: KeyEvent,
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    swarm: &mut SwarmRuntime,
) -> bool {
    let mut clipboard = None;
    let mut shadow = crate::shadow::ShadowRuntime::default();
    handle_agent_station_key_with_clipboard(
        key,
        state,
        vitals,
        codex,
        claude,
        swarm,
        &mut shadow,
        &mut clipboard,
    )
}

fn handle_mouse_event(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    fuzzy_runtime: &mut FuzzySearchRuntime,
    input_state: &mut InputState,
    clipboard: &mut Option<Clipboard>,
    theme: &Theme,
) -> bool {
    handle_mouse_event_with_swarm(
        &SwarmRuntime::default(),
        mouse,
        screen,
        state,
        fuzzy_runtime,
        input_state,
        clipboard,
        theme,
    )
}

fn map_agent_console_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    map_agent_console_mouse_with_swarm(&SwarmRuntime::default(), mouse, screen, state, clamp)
}

fn handle_mouse_down(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    input_state: &mut InputState,
    clipboard: &mut Option<Clipboard>,
    theme: &Theme,
) -> bool {
    handle_mouse_down_with_swarm(
        &SwarmRuntime::default(),
        mouse,
        screen,
        state,
        input_state,
        clipboard,
        theme,
    )
}

fn handle_mouse_drag(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    input_state: &mut InputState,
    clipboard: &mut Option<Clipboard>,
    theme: &Theme,
) -> bool {
    handle_mouse_drag_with_swarm(
        &SwarmRuntime::default(),
        mouse,
        screen,
        state,
        input_state,
        clipboard,
        theme,
    )
}

fn state_for_test() -> AppState {
    let editor = nit_core::Buffer::from_str("editor", "", None);
    let notes = nit_core::Buffer::from_str("notes", "", None);
    AppState::new(std::path::PathBuf::from("."), editor, notes)
}

fn state_for_test_in_workspace(label: &str) -> AppState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let workspace =
        std::env::temp_dir().join(format!("nit-app-{label}-{}-{nanos}", std::process::id()));
    let _ = fs::remove_dir_all(&workspace);
    fs::create_dir_all(&workspace).expect("create workspace");
    let editor = nit_core::Buffer::from_str("editor", "", None);
    let notes = nit_core::Buffer::from_str("notes", "", None);
    AppState::new(workspace, editor, notes)
}

fn seeded_genome_report(
    path: std::path::PathBuf,
    tier: nit_core::GenomeTier,
) -> nit_core::GenomeReport {
    nit_core::GenomeReport {
        file_path: path,
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.42,
        tier,
        recommendations: Vec::new(),
        timestamp_ms: 1,
        grid_size: 32,
        parsimony: Default::default(),
        function_scores: Vec::new(),
    }
}
