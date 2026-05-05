use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};
use nit_core::{actions::Action, AgentOpsTab, AppState, Mode, PaneId, UiSelectionPane};

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
/// Only consumed when the editor is in motion mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum PendingEditorOp {
    Replace,
    FindForward,
    FindBack,
    TillForward,
    TillBack,
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

    /// Vim-style two-key chord recognition. Returns true on the second key of
    /// a matching pair (e.g. `gg`, `dd`, `yy`) when it lands within
    /// `CHORD_TIMEOUT`; otherwise records this key as a half-chord and returns
    /// false. The next call either completes the chord or starts a new one.
    pub(super) fn chord_normal(&mut self, c: char, now: Instant) -> bool {
        if self.normal_last_char == Some(c)
            && now.duration_since(self.normal_last_time) <= CHORD_TIMEOUT
        {
            self.normal_last_char = None;
            return true;
        }
        self.normal_last_char = Some(c);
        self.normal_last_time = now;
        false
    }

    pub(super) fn take_pending_insert(&mut self) -> Option<char> {
        self.pending_insert.take().map(|(c, _)| c)
    }

    /// Insert-mode chord (e.g. `jk` → escape) that ages out after
    /// `CHORD_TIMEOUT`. Returns the deferred char as a literal `InsertChar`
    /// action so the buffer still receives the keystroke when the second key
    /// never arrives.
    pub(super) fn flush_insert_timeout(&mut self) -> Option<Action> {
        let (c, t) = self.pending_insert?;
        if Instant::now().duration_since(t) < CHORD_TIMEOUT {
            return None;
        }
        self.pending_insert = None;
        Some(Action::InsertChar(c))
    }

    pub(super) fn pending_insert_matches(&self, key: &KeyEvent) -> bool {
        let Some((pending, _)) = self.pending_insert else {
            return false;
        };
        matches!(key.code, KeyCode::Char(c) if c == pending)
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

    /// Append a base-10 digit to the in-flight visualizer jump (e.g. typing
    /// `1234<enter>` jumps to generation 1234). Caps digit count at 18 to
    /// stay inside `u64`. Idle resets the timeout.
    pub(super) fn push_visualizer_digit(&mut self, digit: u8) {
        let Some(jump) = self.visualizer_jump.as_mut() else {
            return;
        };
        if jump.digits >= 18 {
            return;
        }
        jump.value = jump.value.saturating_mul(10).saturating_add(digit as u64);
        jump.digits += 1;
        jump.started = Instant::now();
    }

    pub(super) fn pop_visualizer_digit(&mut self) {
        let Some(jump) = self.visualizer_jump.as_mut() else {
            return;
        };
        if jump.digits == 0 {
            return;
        }
        jump.value /= 10;
        jump.digits -= 1;
        jump.started = Instant::now();
    }

    /// Drops the visualizer jump if no key has touched it for
    /// `INSPECTOR_JUMP_TIMEOUT`. Called once per frame from the input pump
    /// so half-typed jumps don't linger indefinitely.
    pub(super) fn expire_visualizer_jump(&mut self) {
        let Some(jump) = self.visualizer_jump.as_ref() else {
            return;
        };
        if Instant::now().duration_since(jump.started) >= INSPECTOR_JUMP_TIMEOUT {
            self.visualizer_jump = None;
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
