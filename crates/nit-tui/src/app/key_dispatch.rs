#![allow(clippy::too_many_arguments)]

use arboard::Clipboard;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use nit_core::{
    actions::Action, apply_action, AgentChannel, AgentMessage, AgentOpsTab, AppKind, AppState,
    JumpEntry, Mode, PaneId, Prompt, SearchMode, YankKind,
};

use crate::{syntax::SyntaxRuntime, vitals::VitalsState};

use super::*;

pub(super) fn handle_editor_buffer_shortcuts(
    key: KeyEvent,
    state: &mut AppState,
    syntax: &mut SyntaxRuntime,
    clipboard: &mut Option<Clipboard>,
) -> bool {
    if state.command_line.is_some()
        || state.prompt.is_some()
        || state.rule_picker.open
        || state.protocol_picker.open
        || state.show_help
        || games_modal_popup_open(state)
    {
        return false;
    }
    if !pane_accepts_text_input(state, state.focus) {
        return false;
    }
    if state.file_tree.open && state.focus == PaneId::Editor {
        return false;
    }
    if handle_jumplist_shortcut(key, state) {
        return true;
    }
    let (buffer_id, is_editor) = match state.focus {
        PaneId::Editor => (state.active_editor_buffer_id, true),
        PaneId::JobOutput if state.agents.dock_tab == AgentOpsTab::Scratchpad => {
            (state.notes_buffer_id, false)
        }
        _ => return false,
    };

    let select_all = matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('a') | KeyCode::Char('A'),
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('\u{1}'),
            modifiers: KeyModifiers::NONE,
            ..
        }
    );
    if select_all {
        state.mode = Mode::Insert;
        if is_editor {
            let buf = state.editor_buffer_mut();
            buf.go_to_top();
            buf.move_home();
            buf.set_selection_anchor();
            buf.go_to_bottom();
            buf.move_end();
            buf.ensure_visible();
        } else {
            let buf = state.notes_buffer_mut();
            buf.go_to_top();
            buf.move_home();
            buf.set_selection_anchor();
            buf.go_to_bottom();
            buf.move_end();
            buf.ensure_visible();
        }
        return true;
    }

    let copy = matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('c') | KeyCode::Char('C'),
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('\u{3}'),
            modifiers: KeyModifiers::NONE,
            ..
        }
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Insert,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    );
    if copy {
        let text = if is_editor {
            state.editor_buffer().yank_selection()
        } else {
            state.notes_buffer().yank_selection()
        };
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            state.yank_kind = if text.contains('\n') {
                YankKind::Line
            } else {
                YankKind::Char
            };
            state.yank = Some(text.clone());
            if let Some(cb) = clipboard.as_mut() {
                let _ = cb.set_text(text);
            }
        }
        return true;
    }

    let cut = matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('x') | KeyCode::Char('X'),
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('\u{18}'),
            modifiers: KeyModifiers::NONE,
            ..
        }
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Delete,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SHIFT)
    );
    if cut {
        if matches!(key.code, KeyCode::Delete) {
            let has_selection = if is_editor {
                state.editor_buffer().selection_range().is_some()
            } else {
                state.notes_buffer().selection_range().is_some()
            };
            if !has_selection {
                return false;
            }
        }

        let (selection_text, changed) = if is_editor {
            let buf = state.editor_buffer_mut();
            let before_version = buf.version();
            let selection_text = buf.yank_selection().filter(|t| !t.is_empty());
            let mut changed = false;
            if selection_text.is_some() {
                changed = buf.delete_selection();
                if changed {
                    buf.ensure_visible();
                }
            }
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
            (selection_text, changed)
        } else {
            let buf = state.notes_buffer_mut();
            let before_version = buf.version();
            let selection_text = buf.yank_selection().filter(|t| !t.is_empty());
            let mut changed = false;
            if selection_text.is_some() {
                changed = buf.delete_selection();
                if changed {
                    buf.ensure_visible();
                }
            }
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
            (selection_text, changed)
        };

        if let Some(text) = selection_text {
            state.yank_kind = if text.contains('\n') {
                YankKind::Line
            } else {
                YankKind::Char
            };
            state.yank = Some(text.clone());
            if let Some(cb) = clipboard.as_mut() {
                let _ = cb.set_text(text);
            }
            if changed {
                state.mode = Mode::Insert;
            }
        }
        return true;
    }

    let paste = matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('v') | KeyCode::Char('V'),
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('\u{16}'),
            modifiers: KeyModifiers::NONE,
            ..
        }
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Insert,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SHIFT)
    );
    if paste {
        state.mode = Mode::Insert;
        let Some(cb) = clipboard.as_mut() else {
            return true;
        };
        let Ok(text) = cb.get_text() else {
            return true;
        };
        let normalized = normalize_buffer_input_text(&text);
        if normalized.is_empty() {
            return true;
        }
        if is_editor {
            let buf = state.editor_buffer_mut();
            let before_version = buf.version();
            buf.break_undo_group();
            buf.insert_str(normalized.as_ref());
            buf.ensure_visible();
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
        } else {
            let buf = state.notes_buffer_mut();
            let before_version = buf.version();
            buf.break_undo_group();
            buf.insert_str(normalized.as_ref());
            buf.ensure_visible();
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
        }
        return true;
    }

    if matches!(key.code, KeyCode::Backspace | KeyCode::Delete) {
        let has_selection = if is_editor {
            state.editor_buffer().selection_range().is_some()
        } else {
            state.notes_buffer().selection_range().is_some()
        };
        if has_selection {
            state.mode = Mode::Insert;
            if is_editor {
                let buf = state.editor_buffer_mut();
                let before_version = buf.version();
                let _ = buf.delete_selection();
                buf.ensure_visible();
                if buf.version() != before_version {
                    syntax.note_buffer_change(buffer_id, buf);
                }
            } else {
                let buf = state.notes_buffer_mut();
                let before_version = buf.version();
                let _ = buf.delete_selection();
                buf.ensure_visible();
                if buf.version() != before_version {
                    syntax.note_buffer_change(buffer_id, buf);
                }
            }
            return true;
        }
    }

    let word_left = matches!(
        key,
        KeyEvent {
            code: KeyCode::Left,
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    );
    if word_left {
        if is_editor {
            let buf = state.editor_buffer_mut();
            buf.move_word_back();
            buf.ensure_visible();
        } else {
            let buf = state.notes_buffer_mut();
            buf.move_word_back();
            buf.ensure_visible();
        }
        return true;
    }

    let word_right = matches!(
        key,
        KeyEvent {
            code: KeyCode::Right,
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    );
    if word_right {
        if is_editor {
            let buf = state.editor_buffer_mut();
            buf.move_word_end();
            buf.ensure_visible();
        } else {
            let buf = state.notes_buffer_mut();
            buf.move_word_end();
            buf.ensure_visible();
        }
        return true;
    }

    let word_backspace = matches!(
        key,
        KeyEvent {
            code: KeyCode::Backspace,
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    );
    if word_backspace {
        state.mode = Mode::Insert;
        if is_editor {
            let buf = state.editor_buffer_mut();
            let before_version = buf.version();
            buf.delete_word_back();
            buf.ensure_visible();
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
        } else {
            let buf = state.notes_buffer_mut();
            let before_version = buf.version();
            buf.delete_word_back();
            buf.ensure_visible();
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
        }
        return true;
    }

    let word_delete = matches!(
        key,
        KeyEvent {
            code: KeyCode::Delete,
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    );
    if word_delete {
        state.mode = Mode::Insert;
        if is_editor {
            let buf = state.editor_buffer_mut();
            let before_version = buf.version();
            buf.delete_word_forward();
            buf.ensure_visible();
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
        } else {
            let buf = state.notes_buffer_mut();
            let before_version = buf.version();
            buf.delete_word_forward();
            buf.ensure_visible();
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
        }
        return true;
    }

    false
}

pub(super) fn handle_paste_event(
    text: &str,
    state: &mut AppState,
    syntax: &mut SyntaxRuntime,
    fuzzy_runtime: &mut FuzzySearchRuntime,
    vitals: &mut VitalsState,
) -> bool {
    if text.is_empty() {
        return false;
    }

    if state.fuzzy_search.open {
        state.fuzzy_search.query.push_str(text);
        fuzzy_runtime.preview_model = None;
        fuzzy_runtime.last_preview_key = None;
        fuzzy_runtime.run_search_for_mode(state);
        return true;
    }

    if state.prompt.is_some()
        || state.rule_picker.open
        || state.protocol_picker.open
        || state.show_help
        || games_modal_popup_open(state)
    {
        return false;
    }

    if let Some(prompt) = state.search_prompt.as_mut() {
        prompt.append_paste(text);
        state.recompute_incremental_search();
        return true;
    }

    if let Some(command_line) = state.command_line.as_mut() {
        command_line.append_paste(text);
        return true;
    }

    if state.agents.artifacts_popup_open {
        let changed = insert_popup_chat_text(state, text);
        if changed {
            state.agents.note_event();
            vitals.record_agent_event(Instant::now());
        }
        return changed;
    }

    if state.focus == PaneId::Notes {
        let changed = insert_chat_input_text(state, text);
        if changed {
            state.agents.note_event();
            vitals.record_agent_event(Instant::now());
        }
        return changed;
    }

    if pane_accepts_text_input(state, state.focus) && state.mode == Mode::Insert {
        return insert_text_into_focused_buffer(state, syntax, text);
    }

    false
}

/// Ctrl-O walks the jumplist back, Ctrl-I walks it forward. Gated on
/// Normal mode + Editor focus so the chord doesn't fight Tab's
/// FocusNextPane / InsertTab role elsewhere. Returns `true` when the
/// chord was consumed.
///
/// All policy (anchor on first back, cross-buffer switching, stale-entry
/// skipping, EOL clamping, empty-ring status) lives in
/// `nit_core::jumplist_apply_step`; this layer's job is purely to
/// recognise the chord and forward it.
pub(super) fn handle_jumplist_shortcut(key: KeyEvent, state: &mut AppState) -> bool {
    if state.focus != PaneId::Editor || state.mode != Mode::Normal {
        return false;
    }
    let is_ctrl_o = matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('o'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } | KeyEvent {
            code: KeyCode::Char('\u{f}'),
            modifiers: KeyModifiers::NONE,
            ..
        }
    );
    let is_ctrl_i = matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('i'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }
    );
    if !is_ctrl_o && !is_ctrl_i {
        return false;
    }
    let dir = if is_ctrl_o {
        nit_core::JumpDirection::Back
    } else {
        nit_core::JumpDirection::Forward
    };
    let _ = nit_core::jumplist_apply_step(state, dir);
    true
}

pub(super) fn map_key_to_action(
    key: KeyEvent,
    state: &AppState,
    input: &mut InputState,
) -> Option<Action> {
    input.expire_visualizer_jump();
    // Prompt confirm takes precedence
    if let Some(Prompt::ConfirmQuit) = state.prompt {
        return match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Some(Action::ConfirmQuitYes),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some(Action::ConfirmQuitNo),
            _ => None,
        };
    }
    if let Some(Prompt::ConfirmCloseBuffer) = state.prompt {
        return match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Some(Action::ConfirmCloseBufferYes),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                Some(Action::ConfirmCloseBufferNo)
            }
            _ => None,
        };
    }
    if let Some(target) = map_focus_hotkey(&key) {
        return Some(Action::FocusPane(target));
    }

    if state.command_line.is_none() && state.prompt.is_none() {
        if is_games_history_open_key(&key, state) {
            return Some(Action::GamesHistoryOpen);
        }
        match key {
            KeyEvent {
                code: KeyCode::Char('p') | KeyCode::Char('P'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                return Some(Action::OpenSearchPopup(SearchMode::Files));
            }
            KeyEvent {
                code: KeyCode::Char('f') | KeyCode::Char('F'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                return Some(Action::OpenSearchPopup(SearchMode::Content));
            }
            _ => {}
        }
    }

    if state.command_line.is_some() {
        return match key.code {
            KeyCode::Esc => Some(Action::CommandPromptCancel),
            KeyCode::Enter => Some(Action::CommandPromptExecute),
            KeyCode::Backspace => Some(Action::CommandPromptBackspace),
            KeyCode::Left => Some(Action::CommandPromptMoveLeft),
            KeyCode::Right => Some(Action::CommandPromptMoveRight),
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                Some(Action::CommandPromptInput(c))
            }
            _ => None,
        };
    }

    if state.search_prompt.is_some() {
        return match key.code {
            KeyCode::Esc => Some(Action::SearchPromptCancel),
            KeyCode::Enter => Some(Action::SearchPromptExecute),
            KeyCode::Backspace => Some(Action::SearchPromptBackspace),
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                Some(Action::SearchPromptInput(c))
            }
            _ => None,
        };
    }

    if is_job_pause_key(&key) {
        return Some(Action::ToggleJobPause);
    }

    if is_petri_show_key(&key, state) {
        return Some(match state.app_kind {
            AppKind::Gol => Action::PetriShow,
            AppKind::Games => Action::GamesShow,
        });
    }

    if is_global_run_key(&key) {
        return Some(match state.app_kind {
            AppKind::Gol => Action::VisualizerRun,
            AppKind::Games => Action::GamesRun,
        });
    }

    if state.app_kind == AppKind::Gol {
        if let Some(action) = visualizer_ctrl_action(&key, state) {
            return Some(action);
        }

        if state.focus == PaneId::Visualizer {
            if let Some(action) = visualizer_inspector_action(&key, state, input) {
                return Some(action);
            }
        }
    }

    if let Some(dir) = ctrl_nav_dir(&key) {
        return Some(Action::FocusPane(focus_by_direction(state, dir)));
    }

    let had_pending_editor_op = input.pending_editor_op.is_some();
    if let Some(action) = handle_editor_pending_op(&key, state, input) {
        return Some(action);
    }
    if had_pending_editor_op {
        // Pending vim op consumed or cancelled the key — don't let it fall through.
        return None;
    }

    if let Some(action) = handle_insert_chords(&key, state, input) {
        return Some(action);
    }

    if state.focus == PaneId::Editor
        && state.mode == Mode::Insert
        && input.pending_insert_matches(&key)
    {
        return None;
    }

    if is_visual_mode(state) {
        match key.code {
            KeyCode::Char('y') => return Some(Action::YankSelection),
            KeyCode::Char('d') => return Some(Action::DeleteSelection),
            KeyCode::Char('v') => return Some(Action::ExitVisual),
            _ => {}
        }
    }

    // T5: `>` / `<` block indent. Lives outside the visual-mode block so
    // `>` in Normal mode also indents the cursor's line (vim's `>>` shape,
    // collapsed to a single keypress here). `modifiers.is_empty() ||
    // modifiers == SHIFT` accepts both crossterm reportings — some
    // terminals attach SHIFT to the shifted glyph, others don't.
    if is_motion_mode(state) {
        match key {
            KeyEvent {
                code: KeyCode::Char('>'),
                modifiers,
                ..
            } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
                return Some(Action::IndentSelection);
            }
            KeyEvent {
                code: KeyCode::Char('<'),
                modifiers,
                ..
            } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
                return Some(Action::DedentSelection);
            }
            _ => {}
        }
    }

    if let Some(action) = handle_normal_chords(&key, state, input) {
        return Some(action);
    }

    if is_command_prompt_open_key(&key) && state.mode == Mode::Normal {
        return Some(Action::CommandPromptOpen);
    }

    // Vim count prefix: digits in motion mode build up `state.pending_count`,
    // which the next motion consumes. `0` is special — it doubles as both a
    // count digit (when a count is already pending) AND the Home motion
    // (when no count is buffered). Order matters: this intercept must
    // precede the main key match, where `0` is wired to `Action::Home`.
    if is_motion_mode(state) {
        if let KeyEvent {
            code: KeyCode::Char(ch),
            modifiers: KeyModifiers::NONE,
            ..
        } = key
        {
            if let Some(digit) = ch.to_digit(10) {
                let is_count_continuation = digit != 0 || state.pending_count.is_some();
                if is_count_continuation {
                    return Some(Action::AppendCountDigit(digit as u8));
                }
            }
        }
    }

    match key {
        KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::Quit),
        KeyEvent {
            code: KeyCode::Char('s'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::Save),
        KeyEvent {
            code: KeyCode::Char('t'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::ToggleFileTree),
        KeyEvent {
            code: KeyCode::Char('z') | KeyCode::Char('Z'),
            modifiers,
            ..
        } if pane_accepts_text_input(state, state.focus)
            && modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
        {
            if modifiers.contains(KeyModifiers::SHIFT) {
                Some(Action::Redo)
            } else {
                Some(Action::Undo)
            }
        }
        KeyEvent {
            code: KeyCode::Char('\u{1a}'),
            modifiers: KeyModifiers::NONE,
            ..
        } if pane_accepts_text_input(state, state.focus) => Some(Action::Undo),
        KeyEvent {
            code: KeyCode::Char('y') | KeyCode::Char('Y'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } if pane_accepts_text_input(state, state.focus) => Some(Action::Redo),
        KeyEvent {
            code: KeyCode::Char('\u{19}'),
            modifiers: KeyModifiers::NONE,
            ..
        } if pane_accepts_text_input(state, state.focus) => Some(Action::Redo),
        key if is_help_toggle_key(&key) => {
            if state.mode != Mode::Insert {
                Some(if state.show_help {
                    Action::HideHelp
                } else {
                    Action::ShowHelp
                })
            } else {
                None
            }
        }
        KeyEvent {
            code: KeyCode::F(3),
            ..
        } if state.mode != Mode::Insert => Some(if state.show_substrate_overlay {
            Action::HideSubstrate
        } else {
            Action::ShowSubstrate
        }),
        // Reliable non-F-key binding: Ctrl+Space. Useful on macOS where
        // function keys often require `fn` or are captured by the OS.
        KeyEvent {
            code: KeyCode::Char(' '),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) && state.mode != Mode::Insert => {
            Some(if state.show_substrate_overlay {
                Action::HideSubstrate
            } else {
                Action::ShowSubstrate
            })
        }
        KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::NONE,
            ..
        } if state.show_substrate_overlay => Some(Action::SubstrateOverlayToggleTab),
        KeyEvent {
            code: KeyCode::Esc, ..
        } if state.show_substrate_overlay => Some(Action::HideSubstrate),
        KeyEvent {
            code: KeyCode::Char('S'),
            modifiers,
            ..
        } if state.focus == PaneId::Editor
            && state.mode != Mode::Insert
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::ToggleSyntax)
        }
        KeyEvent {
            code: KeyCode::Char('g'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::VisualizerToggleSearch),
        KeyEvent {
            code: KeyCode::Char('y'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }
        | KeyEvent {
            code: KeyCode::Char('\u{19}'),
            modifiers: KeyModifiers::NONE,
            ..
        } => Some(Action::VisualizerToggleSeedSource),
        KeyEvent {
            code: KeyCode::Char('n'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::VisualizerSnapshot),
        KeyEvent {
            code: KeyCode::Char('b') | KeyCode::Char('B'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::ToggleDebug),
        KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::SHIFT,
            ..
        } => Some(Action::FocusPrevPane),
        KeyEvent {
            code: KeyCode::Tab, ..
        } => {
            if pane_accepts_text_input(state, state.focus) && state.mode == Mode::Insert {
                Some(Action::InsertTab)
            } else {
                Some(Action::FocusNextPane)
            }
        }
        KeyEvent {
            code: KeyCode::Esc, ..
        } => Some(Action::SwitchMode(Mode::Normal)),
        KeyEvent {
            code: KeyCode::Char('i'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_normal_mode(state) => Some(Action::SwitchMode(Mode::Insert)),
        KeyEvent {
            code: KeyCode::Char('v'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_normal_mode(state) => Some(Action::EnterVisual),
        KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_normal_mode(state) => Some(Action::Append),
        KeyEvent {
            code: KeyCode::Enter,
            ..
        } => Some(Action::InsertNewline),
        KeyEvent {
            code: KeyCode::Backspace,
            ..
        } => Some(Action::Backspace),
        KeyEvent {
            code: KeyCode::Delete,
            ..
        } => Some(Action::Delete),
        KeyEvent {
            code: KeyCode::Left,
            ..
        } => Some(Action::MoveLeft),
        KeyEvent {
            code: KeyCode::Right,
            ..
        } => Some(Action::MoveRight),
        KeyEvent {
            code: KeyCode::Up, ..
        } => Some(Action::MoveUp),
        KeyEvent {
            code: KeyCode::Down,
            ..
        } => Some(Action::MoveDown),
        KeyEvent {
            code: KeyCode::PageUp,
            ..
        } => Some(Action::PageUp),
        KeyEvent {
            code: KeyCode::PageDown,
            ..
        } => Some(Action::PageDown),
        KeyEvent {
            code: KeyCode::Home,
            ..
        } => Some(Action::Home),
        KeyEvent {
            code: KeyCode::End, ..
        } => Some(Action::End),
        KeyEvent {
            code: KeyCode::Char('G'),
            ..
        } if is_motion_mode(state) => Some(Action::GoToBottom),
        KeyEvent {
            code: KeyCode::Char('e'),
            ..
        } if is_motion_mode(state) => Some(Action::MoveWordEnd),
        KeyEvent {
            code: KeyCode::Char('b'),
            ..
        } if is_motion_mode(state) => Some(Action::MoveWordBack),
        KeyEvent {
            code: KeyCode::Char('u'),
            ..
        } if is_normal_mode(state) => Some(Action::Undo),
        KeyEvent {
            code: KeyCode::Char('R'),
            ..
        } if is_normal_mode(state) => Some(Action::Redo),
        KeyEvent {
            code: KeyCode::Char('o'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_normal_mode(state) => Some(Action::OpenLineBelow),
        KeyEvent {
            code: KeyCode::Char('O'),
            ..
        } if is_normal_mode(state) => Some(Action::OpenLineAbove),
        KeyEvent {
            code: KeyCode::Char('$'),
            ..
        } if is_motion_mode(state) => Some(Action::End),
        KeyEvent {
            code: KeyCode::Char('%'),
            ..
        } if is_motion_mode(state) => Some(Action::MatchBracket),
        KeyEvent {
            code: KeyCode::Char('p'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_normal_mode(state) => Some(Action::Paste),
        KeyEvent {
            code: KeyCode::Char('P'),
            ..
        } if is_normal_mode(state) => Some(Action::PasteLineAbove),
        KeyEvent {
            code: KeyCode::Char('h'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => Some(Action::MoveLeft),
        KeyEvent {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => Some(Action::MoveDown),
        KeyEvent {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => Some(Action::MoveUp),
        KeyEvent {
            code: KeyCode::Char('l'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => Some(Action::MoveRight),
        // --- Vim word motions ---
        KeyEvent {
            code: KeyCode::Char('w'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => Some(Action::MoveWordForward),
        KeyEvent {
            code: KeyCode::Char('W'),
            modifiers,
            ..
        } if is_motion_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::MoveBigWordForward)
        }
        KeyEvent {
            code: KeyCode::Char('B'),
            modifiers,
            ..
        } if is_motion_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::MoveBigWordBack)
        }
        KeyEvent {
            code: KeyCode::Char('E'),
            modifiers,
            ..
        } if is_motion_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::MoveBigWordEnd)
        }
        // --- Vim line-anchor motions ---
        KeyEvent {
            code: KeyCode::Char('0'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => Some(Action::Home),
        KeyEvent {
            code: KeyCode::Char('^'),
            modifiers,
            ..
        } if is_motion_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::MoveFirstNonBlank)
        }
        // --- Vim paragraph motions ---
        KeyEvent {
            code: KeyCode::Char('{'),
            modifiers,
            ..
        } if is_motion_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::MoveParagraphUp)
        }
        KeyEvent {
            code: KeyCode::Char('}'),
            modifiers,
            ..
        } if is_motion_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::MoveParagraphDown)
        }
        // --- Vim viewport-anchor motions H / M / L ---
        KeyEvent {
            code: KeyCode::Char('H'),
            modifiers,
            ..
        } if is_motion_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::MoveViewportTop)
        }
        KeyEvent {
            code: KeyCode::Char('M'),
            modifiers,
            ..
        } if is_motion_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::MoveViewportMiddle)
        }
        KeyEvent {
            code: KeyCode::Char('L'),
            modifiers,
            ..
        } if is_motion_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::MoveViewportBottom)
        }
        // --- Vim simple operators in Normal mode ---
        KeyEvent {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_normal_mode(state) => Some(Action::Delete),
        KeyEvent {
            code: KeyCode::Char('X'),
            modifiers,
            ..
        } if is_normal_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::Backspace)
        }
        KeyEvent {
            code: KeyCode::Char('D'),
            modifiers,
            ..
        } if is_normal_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::DeleteToEnd)
        }
        KeyEvent {
            code: KeyCode::Char('C'),
            modifiers,
            ..
        } if is_normal_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::ChangeToEnd)
        }
        KeyEvent {
            code: KeyCode::Char('s'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_normal_mode(state) => Some(Action::SubstituteChar),
        KeyEvent {
            code: KeyCode::Char('J'),
            modifiers,
            ..
        } if is_normal_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::JoinLines)
        }
        KeyEvent {
            code: KeyCode::Char('~'),
            modifiers,
            ..
        } if is_normal_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::ToggleCaseChar)
        }
        KeyEvent {
            code: KeyCode::Char('Y'),
            modifiers,
            ..
        } if is_normal_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::YankLine)
        }
        // --- Vim pending-arg chord starts: r / f / F / t / T / z ---
        KeyEvent {
            code: KeyCode::Char('r'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_normal_mode(state) => {
            input.pending_editor_op = Some(PendingEditorOp::Replace);
            None
        }
        KeyEvent {
            code: KeyCode::Char('f'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => {
            input.pending_editor_op = Some(PendingEditorOp::FindForward);
            None
        }
        KeyEvent {
            code: KeyCode::Char('F'),
            modifiers,
            ..
        } if is_motion_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            input.pending_editor_op = Some(PendingEditorOp::FindBack);
            None
        }
        KeyEvent {
            code: KeyCode::Char('t'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => {
            input.pending_editor_op = Some(PendingEditorOp::TillForward);
            None
        }
        KeyEvent {
            code: KeyCode::Char('T'),
            modifiers,
            ..
        } if is_motion_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            input.pending_editor_op = Some(PendingEditorOp::TillBack);
            None
        }
        KeyEvent {
            code: KeyCode::Char('z'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => {
            input.pending_editor_op = Some(PendingEditorOp::ZMotion);
            None
        }
        // --- Vim repeat-find: ; and , ---
        KeyEvent {
            code: KeyCode::Char(';'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => input
            .last_find
            .map(|(ch, forward, till)| Action::FindChar(ch, forward, till)),
        KeyEvent {
            code: KeyCode::Char(','),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => input
            .last_find
            .map(|(ch, forward, till)| Action::FindChar(ch, !forward, till)),
        // --- Vim scroll: Ctrl-d / Ctrl-u ---
        KeyEvent {
            code: KeyCode::Char('d'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } if is_motion_mode(state) => Some(Action::ScrollHalfPageDown),
        KeyEvent {
            code: KeyCode::Char('u'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } if is_motion_mode(state) => Some(Action::ScrollHalfPageUp),
        // --- Vim in-buffer search: * / # / n / N and `/` prompt ---
        KeyEvent {
            code: KeyCode::Char('*'),
            modifiers,
            ..
        } if is_motion_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::SearchWordForward)
        }
        KeyEvent {
            code: KeyCode::Char('#'),
            modifiers,
            ..
        } if is_motion_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::SearchWordBack)
        }
        KeyEvent {
            code: KeyCode::Char('n'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => Some(Action::SearchNext),
        KeyEvent {
            code: KeyCode::Char('N'),
            modifiers,
            ..
        } if is_motion_mode(state)
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::SearchPrev)
        }
        KeyEvent {
            code: KeyCode::Char('/'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => Some(Action::SearchPromptOpen),
        KeyEvent {
            code: KeyCode::Char(' '),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::ToggleJobPause),
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers,
            ..
        } if (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT)
            && pane_accepts_text_input(state, state.focus)
            && state.mode == Mode::Insert =>
        {
            Some(Action::InsertChar(c))
        }
        _ => None,
    }
}

pub(super) fn apply_action_with_syntax(
    state: &mut AppState,
    syntax: &mut SyntaxRuntime,
    action: Action,
) -> nit_core::state::ActionOutcome {
    let before_focus = state.focus;
    let before_mode = state.mode;
    let before_debug = state.debug;
    let before_editor_id = state.active_editor_buffer_id;
    let before_notes_id = state.notes_buffer_id;
    let editor_version = state.editor_buffer().version();
    let notes_version = state.notes_buffer().version();
    let outcome = apply_action(state, action.clone());
    let after_editor_id = state.active_editor_buffer_id;
    let after_notes_id = state.notes_buffer_id;

    log_action(state, &action, before_focus, before_mode, before_debug);

    if after_editor_id == before_editor_id && state.editor_buffer().version() != editor_version {
        let buf = state.editor_buffer_mut();
        syntax.note_buffer_change(after_editor_id, buf);
    }
    if after_notes_id == before_notes_id && state.notes_buffer().version() != notes_version {
        let buf = state.notes_buffer_mut();
        syntax.note_buffer_change(after_notes_id, buf);
    }

    if matches!(action, Action::ToggleSyntax) {
        syntax.update_config(state.settings.highlight.clone());
        syntax.prime_buffer(after_editor_id, state.editor_buffer(), true);
        syntax.prime_buffer(after_notes_id, state.notes_buffer(), false);
    }
    if matches!(action, Action::OpenFile(_)) {
        // Avoid blocking highlight warmup when hopping files from NITTree.
        syntax.prime_buffer(after_editor_id, state.editor_buffer(), false);
        // Load git HEAD base for diff gutter indicators.
        if let Some(path) = state.editor_buffer().path() {
            if let Some(base) = git_head_content(path) {
                state.editor_buffer_mut().set_git_base(&base);
            }
        }
    }

    outcome
}

pub(super) fn log_action(
    state: &AppState,
    action: &Action,
    before_focus: PaneId,
    before_mode: Mode,
    before_debug: bool,
) {
    match action {
        Action::ToggleDebug => {
            tracing::info!(
                "DEBUG mode {}",
                if state.debug { "ENABLED" } else { "DISABLED" }
            );
        }
        Action::Save | Action::SaveAndNormal => {
            if let Some(status) = &state.status {
                if status.contains("Save failed") || status.contains("No path") {
                    tracing::warn!("SAVE {}", status);
                } else {
                    tracing::info!("SAVE {}", status);
                }
            }
        }
        Action::ConfirmQuitYes => tracing::info!("QUIT confirmed"),
        Action::ConfirmQuitNo => tracing::info!("QUIT canceled"),
        _ => {}
    }

    if !state.debug {
        return;
    }

    if before_focus != state.focus {
        tracing::info!("DEBUG focus {:?} -> {:?}", before_focus, state.focus);
    }
    if before_mode != state.mode {
        tracing::info!("DEBUG mode {:?} -> {:?}", before_mode, state.mode);
    }
    if before_debug != state.debug {
        tracing::info!("DEBUG toggle {}", state.debug);
    }

    tracing::info!("DEBUG action {:?}", action);
}

pub(super) fn handle_clipboard_copy(
    state: &AppState,
    clipboard: &mut Option<Clipboard>,
    action: &Action,
) {
    // Every action that writes the yank register also mirrors to the OS
    // clipboard so `dd` / `D` / `dw` produce text that the user can paste
    // outside nit. The flat `state.yank` field is the single source of
    // truth — these arms only gate which actions count as a copy event.
    if !matches!(
        action,
        Action::YankSelection
            | Action::YankLine
            | Action::DeleteLine
            | Action::DeleteToEnd
            | Action::ChangeToEnd
            | Action::ChangeWordEnd
            | Action::ChangeBigWordEnd
            | Action::ChangeWordBack
            | Action::ChangeBigWordBack
            | Action::ChangeLine
            | Action::DeleteWordForward
            | Action::DeleteWordEnd
            | Action::DeleteWordBack
            | Action::DeleteBigWordForward
            | Action::DeleteBigWordEnd
            | Action::DeleteBigWordBack
    ) {
        return;
    }
    if let (Some(text), Some(cb)) = (state.yank.as_ref(), clipboard.as_mut()) {
        let _ = cb.set_text(text.clone());
    }
}

pub(super) fn handle_selection_autocopy(
    state: &mut AppState,
    clipboard: &mut Option<Clipboard>,
    input_state: &mut InputState,
) {
    if state.mode != Mode::Visual {
        input_state.last_selection = None;
        return;
    }
    let (pane, buffer) = match state.focus {
        PaneId::Editor => (PaneId::Editor, state.editor_buffer()),
        PaneId::JobOutput if state.agents.dock_tab == AgentOpsTab::Scratchpad => {
            (PaneId::JobOutput, state.notes_buffer())
        }
        _ => {
            input_state.last_selection = None;
            return;
        }
    };
    let Some((start, end)) = buffer.selection_range() else {
        input_state.last_selection = None;
        return;
    };
    let signature = SelectionSignature { pane, start, end };
    if input_state.last_selection == Some(signature) {
        return;
    }
    input_state.last_selection = Some(signature);
    if let Some(text) = buffer.yank_selection() {
        state.yank_kind = if text.contains('\n') {
            YankKind::Line
        } else {
            YankKind::Char
        };
        state.yank = Some(text.clone());
        if let Some(cb) = clipboard.as_mut() {
            let _ = cb.set_text(text);
        }
    }
}

pub(super) fn prepare_clipboard_paste(
    state: &mut AppState,
    clipboard: &mut Option<Clipboard>,
    action: &Action,
) {
    if !matches!(action, Action::Paste | Action::PasteLineAbove) || state.yank.is_some() {
        return;
    }
    if let Some(cb) = clipboard.as_mut() {
        if let Ok(text) = cb.get_text() {
            if !text.is_empty() {
                state.yank = Some(text);
                state.yank_kind = if state.yank.as_ref().is_some_and(|t| t.contains('\n')) {
                    YankKind::Line
                } else {
                    YankKind::Char
                };
            }
        }
    }
}
