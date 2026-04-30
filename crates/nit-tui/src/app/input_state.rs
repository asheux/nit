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

#[derive(Copy, Clone, Debug)]
pub(super) enum FocusDir {
    Left,
    Right,
    Up,
    Down,
}

pub(super) fn focus_by_direction(state: &AppState, dir: FocusDir) -> PaneId {
    use FocusDir::*;
    match state.focus {
        PaneId::Notes => match dir {
            Left => PaneId::Notes,
            Right => PaneId::Editor,
            Up => PaneId::Notes,
            Down => PaneId::JobOutput,
        },
        PaneId::JobOutput => match dir {
            Left => PaneId::JobOutput,
            Right => PaneId::Editor,
            Up => PaneId::Notes,
            Down => PaneId::JobOutput,
        },
        PaneId::Visualizer => match dir {
            Left => PaneId::Editor,
            Right => PaneId::Visualizer,
            Up => PaneId::Visualizer,
            Down => PaneId::GateMonitor,
        },
        PaneId::GateMonitor => match dir {
            Left => PaneId::Editor,
            Right => PaneId::GateMonitor,
            Up => PaneId::Visualizer,
            Down => PaneId::GateMonitor,
        },
        PaneId::Editor => {
            let buf = state.editor_buffer();
            let cursor_line = buf.cursor.line.saturating_sub(buf.viewport.offset_line);
            let top_half = cursor_line < buf.viewport.height.saturating_div(2).max(1);
            match dir {
                Left => {
                    if top_half {
                        PaneId::Notes
                    } else {
                        PaneId::JobOutput
                    }
                }
                Right => {
                    if top_half {
                        PaneId::Visualizer
                    } else {
                        PaneId::GateMonitor
                    }
                }
                Up => PaneId::Notes,
                Down => PaneId::JobOutput,
            }
        }
    }
}

/// Pending vim operator waiting for its character argument.
/// Scoped to the editor (only consumed when the editor is in motion mode).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum PendingEditorOp {
    /// `r<c>` — replace char under cursor with `c`.
    Replace,
    /// `f<c>` — jump to next occurrence of `c` on this line.
    FindForward,
    /// `F<c>` — jump to previous occurrence of `c` on this line.
    FindBack,
    /// `t<c>` — jump to one before next `c` on this line.
    TillForward,
    /// `T<c>` — jump to one after previous `c` on this line.
    TillBack,
    /// `z<z|t|b>` — viewport alignment chord.
    ZMotion,
}

pub(super) struct InputState {
    pub(super) normal_last_char: Option<char>,
    pub(super) normal_last_time: Instant,
    pub(super) pending_insert: Option<(char, Instant)>,
    pub(super) deferred_key: Option<KeyEvent>,
    pub(super) visualizer_jump: Option<InspectorJump>,
    pub(super) last_selection: Option<SelectionSignature>,
    pub(super) mouse_select_anchor: Option<MouseSelectAnchor>,
    pub(super) last_ui_selection: Option<UiSelectionSignature>,
    pub(super) pending_editor_op: Option<PendingEditorOp>,
    pub(super) last_find: Option<(char, bool, bool)>,
}

impl InputState {
    pub(super) fn new() -> Self {
        Self {
            normal_last_char: None,
            normal_last_time: Instant::now(),
            pending_insert: None,
            deferred_key: None,
            visualizer_jump: None,
            last_selection: None,
            mouse_select_anchor: None,
            last_ui_selection: None,
            pending_editor_op: None,
            last_find: None,
        }
    }

    pub(super) fn set_pending_editor_op(&mut self, op: PendingEditorOp) {
        self.pending_editor_op = Some(op);
    }

    pub(super) fn clear_pending_editor_op(&mut self) {
        self.pending_editor_op = None;
    }

    pub(super) fn reset_normal(&mut self) {
        self.normal_last_char = None;
    }

    pub(super) fn reset_insert(&mut self) {
        self.pending_insert = None;
    }

    pub(super) fn chord_normal(&mut self, c: char, now: Instant) -> bool {
        if self.normal_last_char == Some(c)
            && now.duration_since(self.normal_last_time) <= CHORD_TIMEOUT
        {
            self.normal_last_char = None;
            true
        } else {
            self.normal_last_char = Some(c);
            self.normal_last_time = now;
            false
        }
    }

    pub(super) fn set_pending_insert(&mut self, c: char, now: Instant) {
        self.pending_insert = Some((c, now));
    }

    pub(super) fn take_pending_insert(&mut self) -> Option<char> {
        self.pending_insert.take().map(|(c, _)| c)
    }

    pub(super) fn flush_insert_timeout(&mut self) -> Option<Action> {
        if let Some((c, t)) = self.pending_insert {
            if Instant::now().duration_since(t) >= CHORD_TIMEOUT {
                self.pending_insert = None;
                return Some(Action::InsertChar(c));
            }
        }
        None
    }

    pub(super) fn pending_insert_matches(&self, key: &KeyEvent) -> bool {
        match (self.pending_insert, key.code) {
            (Some((pending, _)), KeyCode::Char(c)) => pending == c,
            _ => false,
        }
    }

    pub(super) fn defer_key(&mut self, key: KeyEvent) {
        self.deferred_key = Some(key);
    }

    pub(super) fn take_deferred(&mut self) -> Option<KeyEvent> {
        self.deferred_key.take()
    }

    pub(super) fn start_visualizer_jump(&mut self) {
        self.visualizer_jump = Some(InspectorJump {
            value: 0,
            digits: 0,
            started: Instant::now(),
        });
    }

    pub(super) fn clear_visualizer_jump(&mut self) {
        self.visualizer_jump = None;
    }

    pub(super) fn push_visualizer_digit(&mut self, digit: u8) {
        if let Some(jump) = self.visualizer_jump.as_mut() {
            if jump.digits >= 18 {
                return;
            }
            jump.value = jump.value.saturating_mul(10).saturating_add(digit as u64);
            jump.digits += 1;
            jump.started = Instant::now();
        }
    }

    pub(super) fn pop_visualizer_digit(&mut self) {
        if let Some(jump) = self.visualizer_jump.as_mut() {
            if jump.digits == 0 {
                return;
            }
            jump.value /= 10;
            jump.digits -= 1;
            jump.started = Instant::now();
        }
    }

    pub(super) fn visualizer_jump_value(&self) -> Option<u64> {
        self.visualizer_jump.as_ref().map(|jump| jump.value)
    }

    pub(super) fn visualizer_jump_active(&self) -> bool {
        self.visualizer_jump.is_some()
    }

    pub(super) fn expire_visualizer_jump(&mut self) {
        if let Some(jump) = self.visualizer_jump.as_ref() {
            if Instant::now().duration_since(jump.started) >= INSPECTOR_JUMP_TIMEOUT {
                self.visualizer_jump = None;
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct SelectionSignature {
    pub(super) pane: PaneId,
    pub(super) start: usize,
    pub(super) end: usize,
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct MouseSelectAnchor {
    pub(crate) target: MouseSelectTarget,
    pub(crate) line: usize,
    pub(crate) col: usize,
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum MouseSelectTarget {
    Buffer(PaneId),
    Ui(UiSelectionPane),
    ChatInput,
    PopupChatInput,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct UiSelectionSignature {
    pub(super) pane: UiSelectionPane,
    pub(super) start_line: usize,
    pub(super) start_col: usize,
    pub(super) end_line: usize,
    pub(super) end_col: usize,
}

pub(super) struct InspectorJump {
    pub(super) value: u64,
    pub(super) digits: u8,
    pub(super) started: Instant,
}

pub(super) fn is_normal_mode(state: &AppState) -> bool {
    pane_accepts_text_input(state, state.focus) && state.mode == Mode::Normal
}

pub(super) fn is_visual_mode(state: &AppState) -> bool {
    pane_accepts_text_input(state, state.focus) && state.mode == Mode::Visual
}

pub(super) fn is_motion_mode(state: &AppState) -> bool {
    pane_accepts_text_input(state, state.focus) && matches!(state.mode, Mode::Normal | Mode::Visual)
}

pub(super) fn is_insert_editing(state: &AppState) -> bool {
    pane_accepts_text_input(state, state.focus) && state.mode == Mode::Insert
}

pub(super) fn pane_accepts_text_input(_state: &AppState, pane: PaneId) -> bool {
    match pane {
        PaneId::Editor => true,
        PaneId::JobOutput => _state.agents.dock_tab == AgentOpsTab::Scratchpad,
        _ => false,
    }
}
