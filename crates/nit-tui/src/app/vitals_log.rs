#![allow(unused_imports)]
#![allow(clippy::too_many_arguments)]
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::{
    mpsc::{self, Receiver, Sender},
    Arc, Mutex, Weak,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::swarm::{
    chat_clone_base_id, normalize_role_label, GateReport, GateReportGate, SwarmArtifactFocus,
    SwarmRuntime,
};
use crate::{
    claude_runner::{ClaudeRunner, ClaudeRunnerConfig},
    codex_runner::{CodexCommand, CodexRunner, CodexRunnerConfig, CodexRuntimeMode},
    file_tree,
    file_tree_runner::{FileTreeCommand, FileTreeEvent, FileTreeRunner},
    file_watcher::FileWatcher,
    fuzzy_preview_runner::{PreviewEvent, PreviewModel, PreviewRunner},
    fuzzy_search_runner::{
        ContentEvent, ContentSearchRunner, FileIndexRunner, FuzzyCommand, FuzzyEvent,
        FuzzyMatcherRunner, IndexEvent,
    },
    games_petri_dish::GamesPetriDishRuntime,
    layout,
    petri_dish::PetriDishRuntime,
    seed_runtime::SeedRuntime,
    syntax::SyntaxRuntime,
    system_stats::SystemStats,
    theme::Theme,
    vitals::{AgentVitalsState, DiagSeverity, LabVitalsSnapshot, VitalsState},
    widgets::{
        agent_console_view, agent_ops_view, artifacts_history_popup, artifacts_popup, bottom_bar,
        editor_view, file_tree_view, fuzzy_search_popup, games_analysis_popup, games_ca_sim_popup,
        games_match_history_popup, games_replay_popup, games_run_browser_popup,
        games_strategy_popup, games_tm_sim_popup, games_visualizer_view, gate_monitor_view,
        help_overlay, protocol_picker, rule_picker, substrate_overlay, top_bar, visualizer_view,
    },
};
use arboard::Clipboard;
use crossterm::{
    cursor::{SetCursorStyle, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseEvent,
        MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ctrlc::Error as CtrlcError;
use nit_core::{
    actions::Action, apply_action, io as core_io, AgentAlert, AgentAlertSeverity, AgentBusEvent,
    AgentChannel, AgentDiagnosticEvent, AgentMessage, AgentOpsTab, AgentStatus, AppKind, AppState,
    McpConnectionState, MissionPhase, MissionRecord, Mode, PaneId, PatchProposal, PatchStatus,
    Prompt, SavedRunHistoryFilter, SearchMode, UiSelection, UiSelectionPane, YankKind,
    CONSOLE_SCROLL_BOTTOM,
};
use nit_games::config::GamesConfig;
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::Style,
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Terminal,
};

use super::*;

pub(super) fn is_lab_job_running(state: &AppState) -> bool {
    match state.app_kind {
        AppKind::Gol => state.visualizer.running && !state.visualizer.paused,
        AppKind::Games => state.games.running && !state.games.paused,
    }
}

pub(super) fn record_log_line_vitals(vitals: &mut VitalsState, now: Instant, line: &str) {
    if let Some(severity) = log_diag_severity(line) {
        vitals.record_diag_event(now, severity);
    }
    if line_looks_fatal(line) {
        vitals.mark_fatal(now);
    }
}

pub(super) fn append_log_to_agent_diagnostics(state: &mut AppState, line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    let severity = match log_diag_severity(trimmed) {
        Some(DiagSeverity::Error) => AgentAlertSeverity::Error,
        Some(DiagSeverity::Warn) => AgentAlertSeverity::Warn,
        None => AgentAlertSeverity::Info,
    };
    state.agents.diag_events.push(AgentDiagnosticEvent {
        severity,
        source: "runtime".into(),
        message: trimmed.to_string(),
        at: timestamp_label(state),
    });
    if state.agents.diag_events.len() > 512 {
        let drop = state.agents.diag_events.len().saturating_sub(512);
        if drop > 0 {
            state.agents.diag_events.drain(0..drop);
        }
    }
}

pub(super) fn record_agent_bus_vitals(vitals: &mut VitalsState, event: &AgentBusEvent) {
    let now = Instant::now();
    match event {
        // Operator-initiated abort rides TurnFailed but isn't an error,
        // so it shouldn't push the LAB indicator into WARN. Match on the
        // OPERATOR_CANCEL_TURN_MESSAGE sentinel and skip recording — the
        // bus handler already logs it as an Info diag.
        AgentBusEvent::TurnFailed { message, .. }
            if message == nit_core::OPERATOR_CANCEL_TURN_MESSAGE => {}
        AgentBusEvent::TurnFailed { .. } => vitals.record_diag_event(now, DiagSeverity::Error),
        AgentBusEvent::TurnLog { message, .. } => {
            let lowered = message.to_ascii_lowercase();
            if lowered.contains("error") || lowered.contains("failed") {
                vitals.record_diag_event(now, DiagSeverity::Warn);
            }
        }
        AgentBusEvent::AlertAppend { alert } => match alert.severity {
            AgentAlertSeverity::Error => vitals.record_diag_event(now, DiagSeverity::Error),
            AgentAlertSeverity::Warn => vitals.record_diag_event(now, DiagSeverity::Warn),
            AgentAlertSeverity::Info => {}
        },
        AgentBusEvent::DiagnosticAppend { event: diag } => match diag.severity {
            AgentAlertSeverity::Error => vitals.record_diag_event(now, DiagSeverity::Error),
            AgentAlertSeverity::Warn => vitals.record_diag_event(now, DiagSeverity::Warn),
            AgentAlertSeverity::Info => {}
        },
        _ => {}
    }
}

// Strip the `#swarm-<mission>-` segment so log output reads as
// `claude-opus-4-7#clone-01` instead of
// `claude-opus-4-7#swarm-mis-001-clone-01`. Mirrors the compaction used in
// the signals/claims overlay.
pub(super) fn compact_agent_id_for_log(id: &str) -> String {
    let Some((base, rest)) = id.split_once("#swarm-") else {
        return id.to_string();
    };
    let Some(first_dash) = rest.find('-') else {
        return id.to_string();
    };
    let after_first = &rest[first_dash + 1..];
    let Some(second_dash_rel) = after_first.find('-') else {
        return id.to_string();
    };
    let suffix = &after_first[second_dash_rel + 1..];
    if suffix.is_empty() {
        id.to_string()
    } else {
        format!("{base}#{suffix}")
    }
}

pub(super) fn tick_agent_turn_liveness(state: &mut AppState) {
    // Backends can emit periodic `TurnHeartbeat` events. Use those to keep the roster "HB"
    // column honest and to surface stalls when heartbeats stop.
    let now = Instant::now();
    let (agents, active_turns) = {
        let agents_state = &mut state.agents;
        (&mut agents_state.agents, &agents_state.active_turns)
    };
    for agent in agents.iter_mut() {
        if !matches!(agent.status, AgentStatus::Running) {
            continue;
        }
        let Some(turn) = active_turns.get(&agent.id) else {
            continue;
        };
        let age = now
            .checked_duration_since(turn.last_heartbeat_at)
            .or_else(|| now.checked_duration_since(turn.started_at))
            .map(|d| d.as_secs())
            .unwrap_or(0);
        agent.heartbeat_age_secs = age;
    }
}

pub(super) fn is_background_work_active(state: &AppState) -> bool {
    match state.app_kind {
        AppKind::Gol => false,
        AppKind::Games => {
            state.games.running
                || state.games.pending_run
                || state.games.family_building
                || state.games.analysis.running
                || state.games.run_browser.loading
                || state.games.replay.loading
                || state.games.config_preview_pending
                || state.games.pending_analyze.is_some()
                || state.games.pending_run_load.is_some()
                || state.games.pending_replay.is_some()
                || state.status.as_deref().is_some_and(status_looks_busy)
        }
    }
}

pub(super) fn status_looks_busy(status_text: &str) -> bool {
    let lower = status_text.to_ascii_lowercase();
    lower.contains("queued")
        || lower.contains("running")
        || lower.contains("loading")
        || lower.contains("pending")
        || lower.contains("preparing")
        || lower.contains("started")
        || lower.contains("busy")
}

pub(super) fn current_agent_state(state: &AppState) -> AgentVitalsState {
    let enabled = !state.agents.agents.is_empty();
    let connected = matches!(state.agents.mcp.state, McpConnectionState::Connected);
    let active_tasks = state
        .agents
        .agents
        .iter()
        .any(|agent| matches!(agent.status, AgentStatus::Running) || agent.queue_len > 0);
    AgentVitalsState {
        enabled,
        connected,
        active_tasks,
    }
}

pub(super) fn log_diag_severity(line: &str) -> Option<DiagSeverity> {
    let upper = line.to_ascii_uppercase();
    if upper.contains("PANIC") || upper.contains("ERROR") || upper.contains("FAILED") {
        Some(DiagSeverity::Error)
    } else if upper.contains("WARN") {
        Some(DiagSeverity::Warn)
    } else {
        None
    }
}

pub(super) fn line_looks_fatal(line: &str) -> bool {
    let upper = line.to_ascii_uppercase();
    upper.contains("PANIC") || upper.contains("BACKTRACE")
}
