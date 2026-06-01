use super::*;
use crate::{actions::Action, buffer::JumpEntry, io};
use nit_gol::Rule;

use super::jumplist::{apply_step as jumplist_apply_step, JumpDirection};

fn on_off(flag: bool) -> &'static str {
    if flag {
        "ON"
    } else {
        "OFF"
    }
}

/// Closing char for an auto-pair opener, or `None` for chars that don't pair.
/// Kept in sync with the bracket-aware backspace rule in `buffer::edit::delete`
/// (T3): every opener that auto-pairs on insert must also auto-delete its
/// matching closer when the opener is backspaced from `(|)` / `[|]` / `{|}`
/// / `"|"` / `'|'`. Update both tables together.
fn pair_closer(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        '\'' => Some('\''),
        _ => None,
    }
}

fn is_closing_pair(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '"' | '\'')
}

/// InsertChar with bracket/quote auto-pair:
///   - `(`, `[`, `{` → insert `()` etc. with cursor between, unless the next
///     char is a word char (likely the user is wrapping existing text).
///   - `"`, `'` → same, plus skip when adjacent to an identifier char
///     (apostrophes in words like `don't`).
///   - typing the closing char while it's already at the cursor moves the
///     cursor past it instead of inserting a duplicate — needed so `()` typed
///     as `(` then `)` doesn't end up as `()`-then-`)` (i.e. `())`).
fn insert_with_auto_pair(buf: &mut Buffer, c: char) {
    if buf.selection_range().is_some() {
        buf.insert_char(c);
        return;
    }
    if is_closing_pair(c) && buf.peek_char_at_cursor() == Some(c) {
        buf.move_right();
        return;
    }
    let Some(close) = pair_closer(c) else {
        buf.insert_char(c);
        return;
    };
    if !should_auto_pair(buf, c) {
        buf.insert_char(c);
        return;
    }
    buf.insert_pair(c, close);
}

fn should_auto_pair(buf: &Buffer, opener: char) -> bool {
    let next = buf.peek_char_at_cursor();
    // Never pair if the cursor sits right before alphanumerics — the user is
    // most likely wrapping existing text.
    if matches!(next, Some(n) if n.is_alphanumeric()) {
        return false;
    }
    // For quotes only: skip when the previous char is alphanumeric or another
    // quote of the same kind. Catches `don'|t`, ending an empty `""`, etc.
    if matches!(opener, '"' | '\'') {
        let prev = buf.peek_char_before_cursor();
        if matches!(prev, Some(p) if p.is_alphanumeric() || p == opener) {
            return false;
        }
    }
    true
}

fn focus_order_index(focus: PaneId) -> usize {
    PaneId::ALL.iter().position(|p| *p == focus).unwrap_or(0)
}

/// Run `f` on the focused buffer (if any), then call `ensure_visible` so
/// the cursor stays on screen. Returns `true` when a buffer was focused —
/// callers that need to follow up with state mutations (e.g. switching to
/// Insert mode) chain on that bool.
fn with_focused_buffer(state: &mut AppState, f: impl FnOnce(&mut Buffer)) -> bool {
    if let Some(buf) = state.focused_buffer_mut() {
        f(buf);
        buf.ensure_visible();
        true
    } else {
        false
    }
}

/// Take the buffered vim count prefix, returning the value if set or 1 as
/// the default. Used by motion actions that repeat (`5j` → MoveDown × 5).
fn take_motion_count(state: &mut AppState) -> u32 {
    state.pending_count.take().unwrap_or(1)
}

/// Run `f` on the focused buffer N times (where N = the buffered vim
/// count prefix, or 1 if none). Cheaper than repeating the focus +
/// `ensure_visible` work — visibility recomputes only once at the end.
fn repeat_motion(state: &mut AppState, f: impl Fn(&mut Buffer)) {
    let n = take_motion_count(state);
    if let Some(buf) = state.focused_buffer_mut() {
        for _ in 0..n {
            f(buf);
        }
        buf.ensure_visible();
    }
}

/// Switch the global mode and update the focused buffer's selection /
/// insert state to match. Centralises the per-mode buffer side-effects
/// shared by `SwitchMode`, `ToggleMode`, `EnterVisual`, and `ExitVisual`.
fn switch_mode_with_buffer(state: &mut AppState, mode: Mode) {
    state.mode = mode;
    if let Some(buf) = state.focused_buffer_mut() {
        match mode {
            Mode::Normal => {
                buf.exit_insert_mode();
                buf.clear_selection();
            }
            Mode::Visual => {
                buf.set_selection_anchor();
            }
            _ => {
                buf.clear_selection();
            }
        }
    }
}

/// Push the focused editor cursor into the global jumplist. Called from
/// `/`, `n`, `N`, `*`, `#`, `gg`, `G` and any other motion that vim treats
/// as a "jump" — so `Ctrl-O` can walk back through them.
fn push_jump_here(state: &mut AppState) {
    let entry = state.current_jump_entry();
    state.jumplist.push(entry);
}

// `JumpDirection` and the shared apply helper live in
// `super::jumplist` — this arm just routes the action onto them so the
// chord-layer fast path in nit-tui and the action dispatcher stay in
// lockstep on anchor / cross-buffer / EOL-clamp behaviour.

/// vim smart-case for a `/` term: lowercase-only query → case-insensitive,
/// any uppercase → case-sensitive. Mirrors `:set smartcase`.
fn is_smart_case_insensitive(term: &str) -> bool {
    !term.chars().any(|c| c.is_uppercase())
}

/// Width (in spaces) of one indent step inferred from the focused buffer's
/// leading whitespace. Returns `None` when the file is tab-indented (caller
/// should fall back to inserting a literal `\t`). Bounded scan: only the
/// first ~200 lines participate, mirroring the inference cap in
/// `Buffer::indent_unit`. When the buffer has no detectable indentation
/// (fresh file, or one whose first lines are all top-level) the language
/// metadata's `default_indent` provides the fallback so a brand-new Python
/// file's first Tab still expands to four spaces rather than dropping a raw
/// `\t` next to space-indented content.
fn focused_buffer_space_indent_width(buf: &Buffer) -> Option<u8> {
    const SCAN_LINES: usize = 200;
    let scan = buf.lines_len().min(SCAN_LINES);
    let mut widths: Vec<usize> = Vec::new();
    for line_idx in 0..scan {
        let line = buf.line_as_string(line_idx);
        let mut spaces = 0usize;
        let mut saw_tab = false;
        for ch in line.chars() {
            match ch {
                '\t' => {
                    saw_tab = true;
                    break;
                }
                ' ' => spaces += 1,
                _ => break,
            }
        }
        if saw_tab {
            return None;
        }
        let has_content = line
            .chars()
            .nth(spaces)
            .is_some_and(|c| c != '\n' && c != '\r');
        if spaces > 0 && has_content {
            widths.push(spaces);
        }
    }
    if let Some(unit) = widths.iter().copied().reduce(gcd) {
        return Some(unit.clamp(1, 8) as u8);
    }
    let info = buf
        .path()
        .and_then(|p| crate::languages::detect_by_path(p))?;
    let style = info.default_indent?;
    if style.uses_tabs() {
        None
    } else {
        Some(style.width().clamp(1, 8))
    }
}

fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

/// Snapshot the text from the cursor to the end of its line (newline
/// excluded). Returns an empty string when the cursor is past the line's
/// last char — vim's `D` on an empty line is a no-op, so the yank register
/// is left untouched in that case.
fn capture_to_eol(buf: &Buffer) -> String {
    if buf.lines_len() == 0 {
        return String::new();
    }
    let line_idx = buf.cursor.line.min(buf.lines_len() - 1);
    let line = buf.line_as_string(line_idx);
    let chars: Vec<char> = line
        .chars()
        .take_while(|c| *c != '\n' && *c != '\r')
        .collect();
    let col = buf.cursor.col.min(chars.len());
    chars[col..].iter().collect()
}

/// vim incsearch: every keystroke into the `/` prompt re-runs the search
/// from the position the prompt opened at, so adding letters narrows
/// strictly forward instead of drifting from the previous live match.
pub(super) fn run_incremental_search(state: &mut AppState) {
    let Some(prompt) = state.search_prompt.as_ref() else {
        return;
    };
    let term = prompt.input.clone();
    let case_insensitive = is_smart_case_insensitive(&term);
    let Some((buffer_id, cursor)) = prompt.pre_search_cursor else {
        return;
    };
    if buffer_id != state.active_editor_buffer_id {
        return;
    }
    let Some(buf) = state.focused_buffer_mut() else {
        return;
    };
    if term.is_empty() {
        buf.cursor = cursor;
        buf.ensure_visible();
        return;
    }
    let found =
        buf.search_seek_first_match(&term, false, case_insensitive, cursor.line, cursor.col);
    if !found {
        buf.cursor = cursor;
    }
    buf.ensure_visible();
}
pub fn apply_action(state: &mut AppState, action: Action) -> ActionOutcome {
    state.metrics.last_action = Some(action.clone());
    let mut should_exit = false;
    let changed = true;

    // Vim count prefix: every action other than `AppendCountDigit` and the
    // motions that consume the count drops any buffered count. The motion
    // arms below call `take_motion_count` themselves; everything else gets
    // a defensive reset here so a stray `5` followed by `i` doesn't leak
    // a count into the next motion.
    let preserve_count = matches!(
        action,
        Action::AppendCountDigit(_)
            | Action::MoveUp
            | Action::MoveDown
            | Action::MoveLeft
            | Action::MoveRight
            | Action::MoveWordForward
            | Action::MoveWordBack
            | Action::MoveWordEnd
            | Action::MoveBigWordForward
            | Action::MoveBigWordBack
            | Action::MoveBigWordEnd
            | Action::PageUp
            | Action::PageDown
            | Action::ScrollHalfPageDown
            | Action::ScrollHalfPageUp
            | Action::GoToTop
            | Action::GoToBottom
    );

    match action {
        Action::Quit => {
            // Ctrl-Q is the global "exit the app" shortcut — always
            // quits, regardless of launch mode. Diverges from `:q`,
            // which is launch-mode-aware (close-buffer in dir-launch).
            // Confirm-if-dirty applies in both paths.
            if state.has_unsaved_editor_buffers() {
                state.prompt = Some(Prompt::ConfirmQuit);
            } else {
                should_exit = true;
            }
        }
        Action::ConfirmQuitYes => {
            should_exit = true;
        }
        Action::ConfirmQuitNo => {
            state.prompt = None;
        }
        Action::ConfirmCloseBufferYes => {
            state.prompt = None;
            super::cmd_line::close_active_editor_buffer(state);
        }
        Action::ConfirmCloseBufferNo => {
            state.prompt = None;
        }
        Action::Save | Action::SaveAndNormal => {
            let buf = state.editor_buffer_mut();
            if buf.path().is_none() {
                state.status = Some("No path to save".into());
            } else if let Err(e) = io::save_buffer(buf) {
                state.status = Some(format!("Save failed: {e}"));
            } else {
                buf.mark_clean();
                state.status = Some("Saved".into());
                // Request background genome evaluation for the saved file.
                // The TUI layer picks this up and dispatches to GenomeWorker
                // so the UI never blocks on GoL simulation.
                if let Some(file_path) = state.editor_buffer().path().cloned() {
                    state.genome_save_eval_pending = Some(file_path);
                }
            }
            if matches!(action, Action::SaveAndNormal) {
                state.mode = Mode::Normal;
                if let Some(buf) = state.focused_buffer_mut() {
                    buf.exit_insert_mode();
                    buf.clear_selection();
                }
            }
        }
        Action::FocusNextPane => {
            let idx = focus_order_index(state.focus);
            let next = (idx + 1) % PaneId::ALL.len();
            state.focus = PaneId::ALL[next];
        }
        Action::FocusPrevPane => {
            let idx = focus_order_index(state.focus);
            let prev = if idx == 0 {
                PaneId::ALL.len() - 1
            } else {
                idx - 1
            };
            state.focus = PaneId::ALL[prev];
        }
        Action::FocusPane(p) => {
            state.focus = p;
        }
        Action::SwitchMode(m) => switch_mode_with_buffer(state, m),
        Action::ToggleMode => {
            // ToggleMode is special: even if the new mode isn't Normal,
            // we still clear the selection (the legacy semantic).
            let next = state.mode.toggle();
            state.mode = next;
            if let Some(buf) = state.focused_buffer_mut() {
                if next == Mode::Normal {
                    buf.exit_insert_mode();
                }
                buf.clear_selection();
            }
        }
        Action::InsertChar(c) => {
            with_focused_buffer(state, |buf| insert_with_auto_pair(buf, c));
        }
        Action::InsertNewline => {
            with_focused_buffer(state, |buf| buf.insert_newline());
        }
        Action::InsertTab => {
            // T10: when the focused buffer is space-indented (Python, Rust,
            // JS), Tab inserts that many spaces instead of a literal `\t`,
            // so the file doesn't end up with a tab/space mix that Python's
            // tokenizer rejects.
            let indent_text = state.focused_buffer_mut().map(|buf| {
                match focused_buffer_space_indent_width(buf) {
                    Some(width) => " ".repeat(width as usize),
                    None => "\t".to_string(),
                }
            });
            if let Some(text) = indent_text {
                with_focused_buffer(state, |buf| {
                    if text == "\t" {
                        buf.insert_tab();
                    } else {
                        buf.insert_str(&text);
                    }
                });
            }
        }
        Action::EnterVisual => switch_mode_with_buffer(state, Mode::Visual),
        Action::ExitVisual => switch_mode_with_buffer(state, Mode::Normal),
        Action::YankSelection => {
            let yank = if let Some(buf) = state.focused_buffer_mut() {
                let text = buf.yank_selection();
                buf.clear_selection();
                text
            } else {
                None
            };
            match yank {
                Some(text) => {
                    let register = if text.contains('\n') {
                        YankRegister::LineWise(text)
                    } else {
                        YankRegister::CharWise(text)
                    };
                    state.set_yank_register(register);
                }
                None => state.clear_yank_register(),
            }
            state.mode = Mode::Normal;
        }
        Action::YankLine => {
            if let Some(buf) = state.focused_buffer_mut() {
                let text = buf.yank_line();
                state.set_yank_register(YankRegister::LineWise(text));
            }
        }
        Action::DeleteSelection => {
            // Vim's `d` in visual mode yanks the selection before removing
            // it (so `dd` then `p` can re-paste). Linewise vs char-wise is
            // decided by the same `\n` heuristic the yank path uses.
            let captured = if let Some(buf) = state.focused_buffer_mut() {
                let yank = buf.yank_selection();
                let changed = buf.delete_selection();
                if changed {
                    buf.ensure_visible();
                }
                yank
            } else {
                None
            };
            if let Some(text) = captured {
                let register = if text.contains('\n') {
                    YankRegister::LineWise(text)
                } else {
                    YankRegister::CharWise(text)
                };
                state.set_yank_register(register);
            }
            state.mode = Mode::Normal;
        }
        Action::Paste => {
            let register = state.yank_register();
            let is_normal = state.mode == Mode::Normal;
            if let (Some(register), Some(buf)) = (register, state.focused_buffer_mut()) {
                match (is_normal, register) {
                    (true, YankRegister::LineWise(text)) => buf.paste_line_below(&text),
                    (true, YankRegister::CharWise(text)) => {
                        // Seal append + insert as one transaction so a single
                        // undo rewinds the whole paste, like the linewise path.
                        buf.begin_undo_group();
                        buf.append();
                        buf.insert_str(&text);
                        buf.end_undo_group();
                    }
                    (false, register) => buf.insert_str(register.as_str()),
                }
                buf.ensure_visible();
            }
        }
        Action::PasteLineAbove => {
            let register = state.yank_register();
            if let (Some(register), Some(buf)) = (register, state.focused_buffer_mut()) {
                match register {
                    YankRegister::LineWise(text) => buf.paste_line_above(&text),
                    YankRegister::CharWise(text) => {
                        let mut padded = text;
                        if !padded.ends_with('\n') {
                            padded.push('\n');
                        }
                        buf.paste_line_above(&padded);
                    }
                }
                buf.ensure_visible();
            }
        }
        Action::Append => {
            if with_focused_buffer(state, |buf| buf.append()) {
                state.mode = Mode::Insert;
            }
        }
        Action::Backspace => {
            with_focused_buffer(state, |buf| buf.backspace());
        }
        Action::Delete => {
            with_focused_buffer(state, |buf| buf.delete_forward());
        }
        Action::DeleteLine => {
            // vim `dd`: yank the line linewise so `p` later pastes it on a
            // new line and `P` pastes it above.
            let yanked = state.focused_buffer_mut().map(|buf| {
                let text = buf.yank_line();
                buf.delete_line();
                buf.ensure_visible();
                text
            });
            if let Some(text) = yanked {
                state.set_yank_register(YankRegister::LineWise(text));
            }
        }
        Action::MoveUp => {
            repeat_motion(state, |buf| buf.move_up());
        }
        Action::MoveDown => {
            repeat_motion(state, |buf| buf.move_down());
        }
        Action::MoveLeft => {
            repeat_motion(state, |buf| buf.move_left());
        }
        Action::MoveRight => {
            repeat_motion(state, |buf| buf.move_right());
        }
        Action::PageUp => {
            let n = take_motion_count(state);
            with_focused_buffer(state, |buf| {
                let height = buf.viewport.height.max(1);
                for _ in 0..n {
                    buf.page_up(height);
                }
            });
        }
        Action::PageDown => {
            let n = take_motion_count(state);
            with_focused_buffer(state, |buf| {
                let height = buf.viewport.height.max(1);
                for _ in 0..n {
                    buf.page_down(height);
                }
            });
        }
        Action::Home => {
            with_focused_buffer(state, |buf| buf.move_home());
        }
        Action::End => {
            with_focused_buffer(state, |buf| buf.move_end());
        }
        Action::MoveWordEnd => {
            repeat_motion(state, |buf| buf.move_word_end());
        }
        Action::MoveWordBack => {
            repeat_motion(state, |buf| buf.move_word_back());
        }
        Action::GoToTop => {
            // `gg` → line 1; `<N>gg` → line N (1-indexed). Mirrors vim.
            // Push the pre-jump cursor so `Ctrl-O` can come back.
            let count = state.pending_count.take();
            push_jump_here(state);
            with_focused_buffer(state, |buf| match count {
                Some(n) => buf.go_to_line(n as usize),
                None => buf.go_to_top(),
            });
        }
        Action::GoToBottom => {
            let count = state.pending_count.take();
            push_jump_here(state);
            with_focused_buffer(state, |buf| match count {
                Some(n) => buf.go_to_line(n as usize),
                None => buf.go_to_bottom(),
            });
        }
        Action::OpenLineAbove => {
            if with_focused_buffer(state, |buf| buf.open_line_above()) {
                state.mode = Mode::Insert;
            }
        }
        Action::OpenLineBelow => {
            if with_focused_buffer(state, |buf| buf.open_line_below()) {
                state.mode = Mode::Insert;
            }
        }
        Action::Undo => {
            // Skip ensure_visible when undo() returns false — there's
            // nothing to make visible if the stack was empty.
            if let Some(buf) = state.focused_buffer_mut() {
                if buf.undo() {
                    buf.ensure_visible();
                }
            }
        }
        Action::Redo => {
            if let Some(buf) = state.focused_buffer_mut() {
                if buf.redo() {
                    buf.ensure_visible();
                }
            }
        }
        Action::ScrollUp => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.viewport.offset_line = buf.viewport.offset_line.saturating_sub(1);
            }
        }
        Action::ScrollDown => {
            if let Some(buf) = state.focused_buffer_mut() {
                let max_offset = buf.lines_len().saturating_sub(buf.viewport.height.max(1));
                buf.viewport.offset_line =
                    buf.viewport.offset_line.saturating_add(1).min(max_offset);
            }
        }
        Action::ClearLogs => {
            state.logs.clear();
            state.logs_scroll = 0;
        }
        Action::ToggleJobPause => {
            let was_paused = state.job.paused;
            state.job.paused = !state.job.paused;
            if was_paused {
                // Resume log follow.
                state.logs_scroll = 0;
            }
        }
        Action::CommandPromptOpen => {
            state.command_line = Some(CommandLine::new());
        }
        Action::CommandPromptCancel => {
            state.command_line = None;
        }
        Action::CommandPromptBackspace => {
            if let Some(cmd) = state.command_line.as_mut() {
                cmd.backspace();
            }
        }
        Action::CommandPromptMoveLeft => {
            if let Some(cmd) = state.command_line.as_mut() {
                cmd.move_left();
            }
        }
        Action::CommandPromptMoveRight => {
            if let Some(cmd) = state.command_line.as_mut() {
                cmd.move_right();
            }
        }
        Action::CommandPromptExecute => {
            if let Some(cmd) = state.command_line.take() {
                should_exit = handle_command_line(state, &cmd.input);
            }
        }
        Action::CommandPromptInput(ch) => {
            if let Some(cmd) = state.command_line.as_mut() {
                cmd.insert(ch);
            }
        }
        Action::VisualizerReseed => {
            state.visualizer.seed = state.visualizer.seed.wrapping_add(1);
            state.visualizer.pending_reseed = true;
        }
        Action::VisualizerApply => {
            if state.visualizer.seed_search_active {
                state.visualizer.pending_apply = true;
            } else {
                state.visualizer.variant = state.visualizer.variant.wrapping_add(1);
                state.visualizer.pending_reseed = true;
            }
        }
        Action::VisualizerToggleSearch => {
            state.visualizer.seed_search_active = !state.visualizer.seed_search_active;
            state.status = Some(format!(
                "Seed search {}",
                on_off(state.visualizer.seed_search_active)
            ));
        }
        Action::VisualizerToggleWrap => {
            state.visualizer.wrap = !state.visualizer.wrap;
        }
        Action::VisualizerToggleSeedSource => {
            state.visualizer.seed_source = GolSeedSource::Editor;
            state.status = Some("Seed source: Editor (only)".into());
        }
        Action::VisualizerSnapshot => {
            state.visualizer.pending_snapshot = true;
        }
        Action::VisualizerPause => {
            state.visualizer.paused = !state.visualizer.paused;
            state.visualizer.paused_by_attractor = false;
        }
        Action::VisualizerCycleAutoStop => {
            state.visualizer.auto_stop_policy = state.visualizer.auto_stop_policy.next();
            state.status = Some(format!("Auto-stop: {}", state.visualizer.auto_stop_policy));
        }
        Action::VisualizerSpeedUp => {
            state.visualizer.tick_ms = state.visualizer.tick_ms.saturating_sub(10).max(30);
        }
        Action::VisualizerSpeedDown => {
            state.visualizer.tick_ms = (state.visualizer.tick_ms + 10).min(1000);
        }
        Action::VisualizerRun => {
            state.visualizer.pending_run = true;
            state.visualizer.pending_snapshot = true;
            state.status = Some("Petri dish queued".into());
        }
        Action::VisualizerStop => {
            state.visualizer.pending_close = true;
            state.status = Some("Petri dish closing".into());
        }
        Action::GamesRun => {
            state.games.pending_run = true;
            state.status = Some("Games tournament queued".into());
        }
        Action::GamesStop => {
            state.games.pending_close = true;
            state.status = Some("Games tournament closing".into());
        }
        Action::GamesHide => {
            state.games.pending_hide = true;
            state.status = Some("Games tournament hiding".into());
        }
        Action::GamesShow => {
            state.games.pending_show = true;
            state.status = Some("Games tournament showing".into());
        }
        Action::GamesHistoryOpen => {
            open_games_history_popup(state);
        }
        Action::PetriShow => {
            state.visualizer.pending_show = true;
            state.status = Some("Petri dish showing".into());
        }
        Action::VisualizerCycleRenderMode => {
            state.visualizer.seed_plate_mode = state.visualizer.seed_plate_mode.next();
            state.status = Some(format!(
                "Plate mode: {}",
                state.visualizer.seed_plate_mode.label()
            ));
        }
        Action::VisualizerToggleAgeShading => {
            state.visualizer.age_shading = !state.visualizer.age_shading;
            state.status = Some(format!(
                "Age shading: {}",
                on_off(state.visualizer.age_shading)
            ));
        }
        Action::VisualizerToggleTrails => {
            state.visualizer.trails = !state.visualizer.trails;
            state.status = Some(format!("Trails: {}", on_off(state.visualizer.trails)));
        }
        Action::VisualizerToggleBBox => {
            state.visualizer.overlay_bbox = !state.visualizer.overlay_bbox;
            state.status = Some(format!("BBox: {}", on_off(state.visualizer.overlay_bbox)));
        }
        Action::VisualizerToggleHeat => {
            state.visualizer.overlay_heat = !state.visualizer.overlay_heat;
            state.status = Some(format!("Heat: {}", on_off(state.visualizer.overlay_heat)));
        }
        Action::VisualizerToggleScanlines => {
            state.visualizer.scanlines = !state.visualizer.scanlines;
            state.status = Some(format!("Scanlines: {}", on_off(state.visualizer.scanlines)));
        }
        Action::GateMonitorToggleSubView => {
            state.gate_monitor_sub_view = match state.gate_monitor_sub_view {
                GateMonitorSubView::Stats => GateMonitorSubView::FileScores,
                GateMonitorSubView::FileScores => GateMonitorSubView::Live,
                GateMonitorSubView::Live => GateMonitorSubView::Stats,
            };
            state.gate_monitor_scroll = 0;
        }
        Action::GateMonitorSetSubView(target) => {
            if state.gate_monitor_sub_view != target {
                state.gate_monitor_sub_view = target;
                state.gate_monitor_scroll = 0;
            }
        }
        Action::WorkspaceScanStart => {
            // The runner picks this up on the next tick, calls rescan, and
            // sets the status based on the actual outcome (walk found work
            // vs. cache already clean). Setting an eager "evaluating…"
            // status here would stick when the cache is fully fresh and
            // nothing gets queued.
            state.agents.workspace_scan_requested = true;
            // Auto-jump to FILESCORES so the operator can watch tiers
            // update as the scan progresses. No-op if they're already
            // looking at it (mirrors the GateMonitorSetSubView guard).
            if state.gate_monitor_sub_view != crate::state::GateMonitorSubView::FileScores {
                state.gate_monitor_sub_view = crate::state::GateMonitorSubView::FileScores;
                state.gate_monitor_scroll = 0;
            }
        }
        Action::ShowSubstrate => {
            state.show_substrate_overlay = true;
            state.substrate_overlay_scroll = 0;
        }
        Action::HideSubstrate => {
            state.show_substrate_overlay = false;
        }
        Action::SubstrateOverlayToggleTab => {
            state.substrate_overlay_tab = match state.substrate_overlay_tab {
                SubstrateOverlayTab::Signals => SubstrateOverlayTab::Claims,
                SubstrateOverlayTab::Claims => SubstrateOverlayTab::Assumptions,
                SubstrateOverlayTab::Assumptions => SubstrateOverlayTab::Signals,
            };
            state.substrate_overlay_scroll = 0;
        }
        Action::VisualizerCycleSeedView => {
            state.visualizer.seed_view = state.visualizer.seed_view.next();
            state.status = Some(format!("Seed view: {}", state.visualizer.seed_view.label()));
        }
        Action::VisualizerToggleSeedView => {
            state.visualizer.seed_view = state.visualizer.seed_view.toggle_plate();
            state.status = Some(format!("Seed view: {}", state.visualizer.seed_view.label()));
        }
        Action::VisualizerCycleSeedOverlays => {
            cycle_seed_overlays(&mut state.visualizer);
            state.status = Some(format!(
                "Overlays: {}",
                seed_overlay_label(&state.visualizer)
            ));
        }
        Action::VisualizerInspectLeft => {
            move_inspector(state, -1, 0);
        }
        Action::VisualizerInspectRight => {
            move_inspector(state, 1, 0);
        }
        Action::VisualizerInspectUp => {
            move_inspector(state, 0, -1);
        }
        Action::VisualizerInspectDown => {
            move_inspector(state, 0, 1);
        }
        Action::VisualizerInspectHome => {
            set_inspector_pos(state, 0, 0);
        }
        Action::VisualizerInspectEnd => {
            let (w, h) = inspector_dims(state);
            if w > 0 && h > 0 {
                set_inspector_pos(state, w - 1, h - 1);
            }
        }
        Action::VisualizerInspectCenter => {
            let (w, h) = inspector_dims(state);
            if w > 0 && h > 0 {
                set_inspector_pos(state, w / 2, h / 2);
            }
        }
        Action::VisualizerInspectToggle => {
            state.visualizer.inspector_enabled = !state.visualizer.inspector_enabled;
            state.status = Some(format!(
                "Inspector: {}",
                on_off(state.visualizer.inspector_enabled)
            ));
        }
        Action::VisualizerInspectJump(idx) => {
            jump_inspector_to_index(state, idx);
        }
        Action::VisualizerCycleEncoder => {
            state.visualizer.seed_encoder = state.visualizer.seed_encoder.next();
            state.visualizer.pending_reseed = true;
            state.status = Some(format!(
                "Encoder: {}",
                state.visualizer.seed_encoder.label()
            ));
        }
        Action::VisualizerCycleSymmetry => {
            state.visualizer.seed_params.symmetry = state.visualizer.seed_params.symmetry.next();
            state.visualizer.pending_reseed = true;
            state.status = Some(format!(
                "Symmetry: {}",
                state.visualizer.seed_params.symmetry.label()
            ));
        }
        Action::SetGolRuleById(id) => {
            if let Some(named) = state.rule_catalog.find_by_id(&id) {
                apply_rule_selection(state, SelectedRule::from_named(named), true);
            } else {
                state.status = Some(format!("Unknown GoL rule id: {id}"));
            }
        }
        Action::SetGolRuleByString(text) => match Rule::parse(&text) {
            Ok(rule) => {
                let mut selected = SelectedRule::from_rule(rule);
                if let Some(named) = state.rule_catalog.find_by_rule(rule) {
                    selected.id = Some(named.id.clone());
                    selected.name = Some(named.name.clone());
                }
                apply_rule_selection(state, selected, true);
            }
            Err(err) => {
                state.status = Some(format!("Invalid GoL rule '{text}': {err}"));
            }
        },
        Action::OpenRulePicker => {
            if matches!(state.visualizer.rule_mode, RuleMode::Protocol(_)) {
                state.status = Some("Rule picker disabled in protocol mode".into());
            } else {
                state.rule_picker.open = true;
                state.rule_picker.query.clear();
                state.rule_picker.selected = state
                    .rule_catalog
                    .index_of_selected(&state.gol_rule_selected)
                    .unwrap_or(0);
            }
        }
        Action::OpenProtocolPicker => {
            state.protocol_picker.open = true;
            state.protocol_picker.selected = 0;
            state.protocol_picker.custom_input.clear();
            state.protocol_picker.custom_error = None;
            state.protocol_picker.custom_preview = None;
        }
        Action::CloseModal => {
            state.rule_picker.open = false;
            state.rule_picker.query.clear();
            state.rule_picker.selected = 0;
            state.protocol_picker.open = false;
            state.protocol_picker.custom_error = None;
            state.protocol_picker.custom_preview = None;
        }
        Action::ApplySelectedRuleFromPicker => {
            let matches = state.rule_catalog.filter_indices(&state.rule_picker.query);
            if matches.is_empty() {
                state.status = Some("No rules match filter".into());
                state.rule_picker.open = false;
            } else {
                let idx = state
                    .rule_picker
                    .selected
                    .min(matches.len().saturating_sub(1));
                if let Some(named) = state.rule_catalog.get(matches[idx]) {
                    apply_rule_selection(state, SelectedRule::from_named(named), true);
                }
                state.rule_picker.open = false;
            }
        }
        Action::ApplySelectedProtocolFromPicker => {
            let presets = crate::rule_protocol::builtin_protocols(&state.rule_catalog);
            let idx = state
                .protocol_picker
                .selected
                .min(presets.len().saturating_add(1).saturating_sub(1));
            if idx < presets.len() {
                let preset = &presets[idx];
                apply_protocol_selection(state, preset.mode.clone(), Some(preset.name.clone()));
                state.status = Some(format!("Protocol set to {}", preset.name));
                state.protocol_picker.open = false;
                state.protocol_picker.custom_error = None;
            } else {
                match crate::rule_protocol::parse_protocol_spec(
                    &state.protocol_picker.custom_input,
                    &state.rule_catalog,
                ) {
                    Ok(mut protocol) => {
                        protocol.reset();
                        apply_protocol_selection(
                            state,
                            RuleMode::Protocol(protocol),
                            Some("Custom".into()),
                        );
                        state.status = Some("Protocol set to Custom".into());
                        state.protocol_picker.open = false;
                        state.protocol_picker.custom_error = None;
                    }
                    Err(err) => {
                        state.protocol_picker.custom_error = Some(err);
                    }
                }
            }
        }
        Action::ToggleSyntax => {
            state.settings.highlight.enabled = !state.settings.highlight.enabled;
        }
        Action::ToggleDebug => {
            state.debug = !state.debug;
            state.status = Some(format!("Debug {}", on_off(state.debug)));
        }
        Action::ToggleFileTree => {
            state.file_tree.open = !state.file_tree.open;
            if state.file_tree.open {
                state.file_tree.root = state.workspace_root.clone();
                state.focus = PaneId::Editor;
                state.mode = Mode::Normal;
            }
        }
        Action::OpenSearchPopup(mode) => {
            state.show_help = false;
            state.rule_picker.open = false;
            state.protocol_picker.open = false;
            state.fuzzy_search.open(mode, state.workspace_root.clone());
            state.focus = PaneId::Editor;
            state.mode = Mode::Normal;
        }
        Action::CloseSearchPopup => {
            state.fuzzy_search.close();
        }
        Action::OpenFile(path) => {
            // Snapshot where we're leaving so `Ctrl-O` can come back to
            // it. NITTree row activation, the Ctrl-P fuzzy picker, the
            // Ctrl-F content picker, and `:e` all funnel through this
            // arm, so a single push here covers every cross-buffer
            // entry point. The push is gated on actually changing
            // buffers — re-opening the already-focused file would
            // otherwise spam the ring with no-op jumps.
            let pre_open = state.current_jump_entry();
            if let Some(buffer_id) = state.find_editor_buffer_by_path(&path) {
                if buffer_id != state.active_editor_buffer_id {
                    state.jumplist.push(pre_open);
                }
                state.active_editor_buffer_id = buffer_id;
                state.focus = PaneId::Editor;
                state.mode = Mode::Normal;
                state.visualizer.pending_reseed = true;
                state.status = Some(format!("Opened {}", path.display()));
            } else {
                match io::load_to_string(&path) {
                    Ok(content) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "untitled".into());
                        let buf = Buffer::from_str(name, &content, Some(path.clone()));
                        // The active slot is "blank-and-disposable" only
                        // when it has no path (the fresh untitled buffer
                        // nit launches with when no file argument is
                        // given). A clean *real* file is preserved so the
                        // jumplist can walk back to it — without this, a
                        // Ctrl-P open from a never-edited file would
                        // silently evict that file from `buffers` and
                        // strand the Ctrl-O return.
                        let active_is_initial_blank = state.editor_buffer().path().is_none()
                            && !state.editor_buffer().is_dirty();
                        state.jumplist.push(pre_open);
                        if active_is_initial_blank {
                            state.buffers[state.active_editor_buffer_id] = buf;
                        } else {
                            state.buffers.push(buf);
                            state.active_editor_buffer_id = state.buffers.len() - 1;
                        }
                        state.focus = PaneId::Editor;
                        state.mode = Mode::Normal;
                        state.visualizer.pending_reseed = true;
                        state.status = Some(format!("Opened {}", path.display()));
                    }
                    Err(err) => {
                        state.status = Some(format!("Open failed: {err}"));
                    }
                }
            }
        }
        Action::ShowHelp => {
            state.show_help = true;
            state.help_scroll = 0;
        }
        Action::HideHelp => {
            state.show_help = false;
            state.help_scroll = 0;
            if let Some(selection) = state.ui_selection {
                if matches!(selection.pane, UiSelectionPane::HelpPopup) {
                    state.ui_selection = None;
                }
            }
        }
        Action::MoveWordForward => {
            repeat_motion(state, |buf| buf.move_word_forward());
        }
        Action::MoveBigWordForward => {
            repeat_motion(state, |buf| buf.move_big_word_forward());
        }
        Action::MoveBigWordBack => {
            repeat_motion(state, |buf| buf.move_big_word_back());
        }
        Action::MoveBigWordEnd => {
            repeat_motion(state, |buf| buf.move_big_word_end());
        }
        Action::DeleteWordForward => {
            let n = take_motion_count(state);
            with_focused_buffer(state, |buf| {
                for _ in 0..n {
                    buf.delete_word_forward();
                }
            });
        }
        Action::DeleteWordEnd => {
            let n = take_motion_count(state);
            with_focused_buffer(state, |buf| {
                for _ in 0..n {
                    buf.delete_word_end();
                }
            });
        }
        Action::DeleteWordBack => {
            let n = take_motion_count(state);
            with_focused_buffer(state, |buf| {
                for _ in 0..n {
                    buf.delete_word_back();
                }
            });
        }
        Action::DeleteBigWordForward => {
            let n = take_motion_count(state);
            with_focused_buffer(state, |buf| {
                for _ in 0..n {
                    buf.delete_big_word_forward();
                }
            });
        }
        Action::DeleteBigWordEnd => {
            let n = take_motion_count(state);
            with_focused_buffer(state, |buf| {
                for _ in 0..n {
                    buf.delete_big_word_end();
                }
            });
        }
        Action::DeleteBigWordBack => {
            let n = take_motion_count(state);
            with_focused_buffer(state, |buf| {
                for _ in 0..n {
                    buf.delete_big_word_back();
                }
            });
        }
        Action::JumpBack => {
            let _ = jumplist_apply_step(state, JumpDirection::Back);
        }
        Action::JumpForward => {
            let _ = jumplist_apply_step(state, JumpDirection::Forward);
        }
        Action::MatchBracket => {
            // `%` is a vim "jump" — record the origin so `Ctrl-O` can walk
            // back. The push happens unconditionally even when the motion
            // is a no-op (cursor not on a bracket, no bracket on the line)
            // because vim itself records the position before consulting
            // the partner table; the dedup in `JumpList::push` prevents a
            // run of no-op presses from filling the ring.
            push_jump_here(state);
            with_focused_buffer(state, |buf| buf.match_bracket());
        }
        Action::IndentSelection => {
            // Vim's `>` in visual mode exits to Normal once the shift lands,
            // mirroring the YankSelection / DeleteSelection pattern. The
            // selection is cleared so the operator can keep moving without
            // re-selecting (vim's `gv` re-selects on demand).
            let was_visual = state.mode == Mode::Visual;
            with_focused_buffer(state, |buf| {
                let _ = buf.indent_selection();
                if was_visual {
                    buf.clear_selection();
                }
            });
            if was_visual {
                state.mode = Mode::Normal;
            }
        }
        Action::DedentSelection => {
            let was_visual = state.mode == Mode::Visual;
            with_focused_buffer(state, |buf| {
                let _ = buf.dedent_selection();
                if was_visual {
                    buf.clear_selection();
                }
            });
            if was_visual {
                state.mode = Mode::Normal;
            }
        }
        Action::UppercaseSelection => {
            with_focused_buffer(state, |buf| buf.uppercase_selection());
            state.mode = Mode::Normal;
        }
        Action::LowercaseSelection => {
            with_focused_buffer(state, |buf| buf.lowercase_selection());
            state.mode = Mode::Normal;
        }
        Action::MoveFirstNonBlank => {
            with_focused_buffer(state, |buf| buf.move_first_non_blank());
        }
        Action::MoveLastNonBlank => {
            with_focused_buffer(state, |buf| buf.move_last_non_blank());
        }
        Action::MoveParagraphUp => {
            with_focused_buffer(state, |buf| buf.move_paragraph_up());
        }
        Action::MoveParagraphDown => {
            with_focused_buffer(state, |buf| buf.move_paragraph_down());
        }
        Action::MoveViewportTop => {
            with_focused_buffer(state, |buf| buf.move_viewport_top());
        }
        Action::MoveViewportMiddle => {
            with_focused_buffer(state, |buf| buf.move_viewport_middle());
        }
        Action::MoveViewportBottom => {
            with_focused_buffer(state, |buf| buf.move_viewport_bottom());
        }
        Action::DeleteToEnd => {
            let yanked = state.focused_buffer_mut().map(|buf| {
                let text = capture_to_eol(buf);
                buf.delete_to_end();
                buf.ensure_visible();
                text
            });
            if let Some(text) = yanked.filter(|t| !t.is_empty()) {
                state.set_yank_register(YankRegister::CharWise(text));
            }
        }
        Action::ChangeToEnd => {
            // ChangeToEnd swaps to Insert mode unconditionally, mirroring
            // vim's `C` semantics (no-op buffer + still-in-Insert is the
            // documented behaviour when there's no focused buffer).
            let yanked = state.focused_buffer_mut().map(|buf| {
                let text = capture_to_eol(buf);
                buf.delete_to_end();
                buf.ensure_visible();
                text
            });
            if let Some(text) = yanked.filter(|t| !t.is_empty()) {
                state.set_yank_register(YankRegister::CharWise(text));
            }
            state.mode = Mode::Insert;
        }
        Action::ChangeWordEnd => {
            // `cw` / `ce`: vim's `cw` deviates from `de` by *not* advancing
            // past a single-char word edge (see `:h cw`), so this routes
            // through `delete_word_change` rather than `delete_word_end`.
            // Count-aware: `3cw` concatenates the three deletions into one
            // CharWise yank.
            let n = take_motion_count(state);
            let yanked = state.focused_buffer_mut().map(|buf| {
                let mut text = String::new();
                for _ in 0..n {
                    text.push_str(&buf.delete_word_change(false));
                }
                buf.ensure_visible();
                text
            });
            if let Some(text) = yanked.filter(|t| !t.is_empty()) {
                state.set_yank_register(YankRegister::CharWise(text));
            }
            state.mode = Mode::Insert;
        }
        Action::ChangeBigWordEnd => {
            let n = take_motion_count(state);
            let yanked = state.focused_buffer_mut().map(|buf| {
                let mut text = String::new();
                for _ in 0..n {
                    text.push_str(&buf.delete_word_change(true));
                }
                buf.ensure_visible();
                text
            });
            if let Some(text) = yanked.filter(|t| !t.is_empty()) {
                state.set_yank_register(YankRegister::CharWise(text));
            }
            state.mode = Mode::Insert;
        }
        Action::ChangeWordBack => {
            let n = take_motion_count(state);
            let yanked = state.focused_buffer_mut().map(|buf| {
                let mut text = String::new();
                for _ in 0..n {
                    text.push_str(&buf.delete_word_back());
                }
                buf.ensure_visible();
                text
            });
            if let Some(text) = yanked.filter(|t| !t.is_empty()) {
                state.set_yank_register(YankRegister::CharWise(text));
            }
            state.mode = Mode::Insert;
        }
        Action::ChangeBigWordBack => {
            let n = take_motion_count(state);
            let yanked = state.focused_buffer_mut().map(|buf| {
                let mut text = String::new();
                for _ in 0..n {
                    text.push_str(&buf.delete_big_word_back());
                }
                buf.ensure_visible();
                text
            });
            if let Some(text) = yanked.filter(|t| !t.is_empty()) {
                state.set_yank_register(YankRegister::CharWise(text));
            }
            state.mode = Mode::Insert;
        }
        Action::ChangeLine => {
            // `cc`: linewise yank, drop the line, open a fresh one above the
            // cursor position so the leading indent matches the prior line's
            // (`open_line_above` reuses the line-above indent the way vim's
            // `cc` does). Single undo group via the buffer methods' shared
            // edit-tracking.
            let yanked = state.focused_buffer_mut().map(|buf| {
                let text = buf.yank_line();
                buf.delete_line();
                buf.open_line_above();
                buf.ensure_visible();
                text
            });
            if let Some(text) = yanked {
                state.set_yank_register(YankRegister::LineWise(text));
            }
            state.mode = Mode::Insert;
        }
        Action::SubstituteChar => {
            let yanked = state.focused_buffer_mut().and_then(|buf| {
                let captured = buf.peek_char_at_cursor().map(|c| c.to_string());
                buf.delete_forward();
                buf.ensure_visible();
                captured
            });
            if let Some(text) = yanked.filter(|t| !t.is_empty()) {
                state.set_yank_register(YankRegister::CharWise(text));
            }
            state.mode = Mode::Insert;
        }
        Action::SubstituteLine => {
            let yanked = state.focused_buffer_mut().map(|buf| {
                let text = buf.yank_line();
                buf.substitute_line();
                buf.ensure_visible();
                text
            });
            if let Some(text) = yanked {
                state.set_yank_register(YankRegister::LineWise(text));
            }
            state.mode = Mode::Insert;
        }
        Action::JoinLines => {
            with_focused_buffer(state, |buf| buf.join_lines());
        }
        Action::ToggleCaseChar => {
            with_focused_buffer(state, |buf| buf.toggle_case_char());
        }
        Action::ReplaceChar(c) => {
            with_focused_buffer(state, |buf| buf.replace_char(c));
        }
        Action::FindChar(ch, forward, till) => {
            with_focused_buffer(state, |buf| {
                buf.find_char_in_line(ch, forward, till);
            });
        }
        Action::ScrollHalfPageDown => {
            repeat_motion(state, |buf| buf.scroll_half_page_down());
        }
        Action::ScrollHalfPageUp => {
            repeat_motion(state, |buf| buf.scroll_half_page_up());
        }
        Action::AppendCountDigit(digit) => {
            let current = state.pending_count.unwrap_or(0);
            // Cap at 99_999 — defends against a stuck digit key producing
            // a 4-billion-iteration motion. Higher caps don't add real-
            // world value: nobody types `100000j` on purpose.
            let next = current.saturating_mul(10).saturating_add(digit as u32);
            state.pending_count = Some(next.min(99_999));
        }
        Action::CenterViewportOnCursor => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.center_viewport_on_cursor();
            }
        }
        Action::ViewportTopOnCursor => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.viewport_top_on_cursor();
            }
        }
        Action::ViewportBottomOnCursor => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.viewport_bottom_on_cursor();
            }
        }
        Action::SearchWordForward => {
            let word = state.focused_buffer_mut().and_then(|b| b.word_at_cursor());
            if let Some(term) = word {
                push_jump_here(state);
                state.editor_search.term = Some(term.clone());
                state.editor_search.whole_word = true;
                state.editor_search.forward = true;
                let case_insensitive = is_smart_case_insensitive(&term);
                if let Some(buf) = state.focused_buffer_mut() {
                    buf.search_next_match_opt(&term, true, case_insensitive);
                    buf.ensure_visible();
                }
                state.status = Some(format!("/\\<{term}\\>"));
            } else {
                state.status = Some("No word under cursor".into());
            }
        }
        Action::SearchWordBack => {
            let word = state.focused_buffer_mut().and_then(|b| b.word_at_cursor());
            if let Some(term) = word {
                push_jump_here(state);
                state.editor_search.term = Some(term.clone());
                state.editor_search.whole_word = true;
                state.editor_search.forward = false;
                let case_insensitive = is_smart_case_insensitive(&term);
                if let Some(buf) = state.focused_buffer_mut() {
                    buf.search_prev_match_opt(&term, true, case_insensitive);
                    buf.ensure_visible();
                }
                state.status = Some(format!("?\\<{term}\\>"));
            } else {
                state.status = Some("No word under cursor".into());
            }
        }
        Action::SearchNext => {
            let term = state.editor_search.term.clone();
            let whole_word = state.editor_search.whole_word;
            let forward = state.editor_search.forward;
            if let Some(term) = term {
                push_jump_here(state);
                let case_insensitive = is_smart_case_insensitive(&term);
                if let Some(buf) = state.focused_buffer_mut() {
                    let found = if forward {
                        buf.search_next_match_opt(&term, whole_word, case_insensitive)
                    } else {
                        buf.search_prev_match_opt(&term, whole_word, case_insensitive)
                    };
                    buf.ensure_visible();
                    if !found {
                        state.status = Some(format!("Pattern not found: {term}"));
                    }
                }
            } else {
                state.status = Some("No previous search".into());
            }
        }
        Action::SearchPrev => {
            let term = state.editor_search.term.clone();
            let whole_word = state.editor_search.whole_word;
            let forward = state.editor_search.forward;
            if let Some(term) = term {
                push_jump_here(state);
                let case_insensitive = is_smart_case_insensitive(&term);
                if let Some(buf) = state.focused_buffer_mut() {
                    let found = if forward {
                        buf.search_prev_match_opt(&term, whole_word, case_insensitive)
                    } else {
                        buf.search_next_match_opt(&term, whole_word, case_insensitive)
                    };
                    buf.ensure_visible();
                    if !found {
                        state.status = Some(format!("Pattern not found: {term}"));
                    }
                }
            } else {
                state.status = Some("No previous search".into());
            }
        }
        Action::SearchClear => {
            state.editor_search.clear();
        }
        Action::SearchPromptOpen => {
            // Capture the cursor we started from so `Esc` can put it back
            // and so `/<term><Enter>` can push it onto the jumplist for
            // `Ctrl-O` to walk back to.
            let buffer_id = state.active_editor_buffer_id;
            let cursor = state.editor_buffer().cursor;
            state.search_prompt = Some(SearchPrompt::with_origin(buffer_id, cursor));
        }
        Action::SearchPromptCancel => {
            // Restore the pre-prompt cursor (vim incsearch semantics): every
            // keystroke moved the cursor onto the first match, so cancelling
            // would otherwise leave it parked on the last incremental hit.
            if let Some(prompt) = state.search_prompt.take() {
                if let Some((buffer_id, cursor)) = prompt.pre_search_cursor {
                    if buffer_id == state.active_editor_buffer_id {
                        if let Some(buf) = state.focused_buffer_mut() {
                            buf.cursor = cursor;
                            buf.ensure_visible();
                        }
                    }
                }
            }
        }
        Action::SearchPromptBackspace => {
            if let Some(p) = state.search_prompt.as_mut() {
                p.backspace();
            }
            run_incremental_search(state);
        }
        Action::SearchPromptInput(ch) => {
            if let Some(p) = state.search_prompt.as_mut() {
                p.insert(ch);
            }
            run_incremental_search(state);
        }
        Action::SearchPromptExecute => {
            if let Some(prompt) = state.search_prompt.take() {
                let term = prompt.input;
                if !term.is_empty() {
                    // Push the cursor we started from so Ctrl-O can return.
                    if let Some((_buffer_id, cursor)) = prompt.pre_search_cursor {
                        state.jumplist.push(JumpEntry::new(
                            state.active_editor_buffer_id,
                            cursor.line,
                            cursor.col,
                        ));
                    } else {
                        push_jump_here(state);
                    }
                    state.editor_search.term = Some(term.clone());
                    state.editor_search.whole_word = false;
                    state.editor_search.forward = true;
                    let case_insensitive = is_smart_case_insensitive(&term);
                    if let Some(buf) = state.focused_buffer_mut() {
                        let found = buf.search_next_match_opt(&term, false, case_insensitive);
                        buf.ensure_visible();
                        if !found {
                            state.status = Some(format!("Pattern not found: {term}"));
                        } else {
                            state.status = Some(format!("/{term}"));
                        }
                    }
                }
            }
        }
        Action::GotoDefinition => {
            open_definition_popup(state);
        }
        Action::ToggleTerminalPane => {
            state.terminal_pane_active = !state.terminal_pane_active;
            // Focus the chat slot so the terminal owns input; restore the
            // editor on toggle-off. The runner reconciles the PtySession.
            if state.terminal_pane_active {
                state.focus = PaneId::Notes;
                state.mode = Mode::Normal;
            } else {
                state.focus = PaneId::Editor;
            }
        }
        Action::ToggleTerminalPopup => {
            // Record the intent only — the event loop pins the cwd and
            // reconciles the persistent PtySession (close hides, quit kills),
            // since nit-core owns no subprocess.
            state.terminal_popup.toggle_requested = true;
        }
    }

    if !preserve_count {
        state.pending_count = None;
    }

    ActionOutcome {
        should_exit,
        state_changed: changed,
    }
}

/// Hard cap on the snippet captured for the goto-definition popup. The view is
/// scrollable, so this only bounds how far past the definition line we read.
const DEFINITION_SNIPPET_LINES: usize = 80;

/// `gd`: resolve the identifier under the editor cursor to its first same-file
/// definition-shaped line and open the scrollable popup. No-ops (with a status
/// hint) when there is no identifier or no match.
fn open_definition_popup(state: &mut AppState) {
    let Some(word) = state.editor_buffer().word_at_cursor() else {
        state.status = Some("No identifier under cursor".into());
        return;
    };
    let content = state.editor_buffer().content_as_string();
    let lines: Vec<&str> = content.lines().collect();
    let Some(idx) = find_definition_line(&lines, &word) else {
        state.status = Some(format!("No definition found: {word}"));
        return;
    };
    let path = definition_display_path(state);
    let snippet = lines
        .iter()
        .skip(idx)
        .take(DEFINITION_SNIPPET_LINES)
        .map(|line| line.to_string())
        .collect();
    let title = format!("Definition: {word} ({path}:{})", idx + 1);
    state.definition_popup = Some(DefinitionView {
        title,
        path,
        start_line: idx + 1,
        lines: snippet,
        scroll: 0,
    });
}

/// Workspace-relative display path for the active editor buffer, falling back
/// to `buffer` for an unsaved scratch buffer with no path.
fn definition_display_path(state: &AppState) -> String {
    state
        .editor_buffer()
        .path()
        .map(|p| {
            p.strip_prefix(&state.workspace_root)
                .unwrap_or(p)
                .display()
                .to_string()
        })
        .unwrap_or_else(|| "buffer".to_string())
}
