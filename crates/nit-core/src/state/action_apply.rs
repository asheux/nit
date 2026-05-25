use super::*;
use crate::{actions::Action, io};
use nit_gol::Rule;

fn on_off(flag: bool) -> &'static str {
    if flag {
        "ON"
    } else {
        "OFF"
    }
}

/// Closing char for an auto-pair opener, or `None` for chars that don't pair.
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
            with_focused_buffer(state, |buf| buf.insert_tab());
        }
        Action::EnterVisual => switch_mode_with_buffer(state, Mode::Visual),
        Action::ExitVisual => switch_mode_with_buffer(state, Mode::Normal),
        Action::YankSelection => {
            let yank = if let Some(buf) = state.focused_buffer_mut() {
                let yank = buf.yank_selection();
                buf.clear_selection();
                yank
            } else {
                None
            };
            if let Some(text) = yank {
                state.yank_kind = if text.contains('\n') {
                    YankKind::Line
                } else {
                    YankKind::Char
                };
                state.yank = Some(text);
            } else {
                state.yank = None;
                state.yank_kind = YankKind::Char;
            }
            state.mode = Mode::Normal;
        }
        Action::YankLine => {
            if let Some(buf) = state.focused_buffer_mut() {
                state.yank = Some(buf.yank_line());
                state.yank_kind = YankKind::Line;
            }
        }
        Action::DeleteSelection => {
            if let Some(buf) = state.focused_buffer_mut() {
                if buf.delete_selection() {
                    buf.ensure_visible();
                }
            }
            state.mode = Mode::Normal;
        }
        Action::Paste => {
            let yank = state.yank.clone();
            let is_normal = state.mode == Mode::Normal;
            let yank_kind = state.yank_kind;
            if let (Some(yank), Some(buf)) = (yank, state.focused_buffer_mut()) {
                if is_normal && yank_kind == YankKind::Line {
                    buf.paste_line_below(&yank);
                } else {
                    if is_normal {
                        buf.append();
                    }
                    buf.insert_str(&yank);
                }
                buf.ensure_visible();
            }
        }
        Action::PasteLineAbove => {
            let yank = state.yank.clone();
            let yank_kind = state.yank_kind;
            if let (Some(yank), Some(buf)) = (yank, state.focused_buffer_mut()) {
                if yank_kind == YankKind::Line {
                    buf.paste_line_above(&yank);
                } else {
                    let mut text = yank;
                    if !text.ends_with('\n') {
                        text.push('\n');
                    }
                    buf.paste_line_above(&text);
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
            with_focused_buffer(state, |buf| buf.delete_line());
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
            let count = state.pending_count.take();
            with_focused_buffer(state, |buf| match count {
                Some(n) => buf.go_to_line(n as usize),
                None => buf.go_to_top(),
            });
        }
        Action::GoToBottom => {
            // `G` → last line; `<N>G` → line N (1-indexed). Mirrors vim.
            let count = state.pending_count.take();
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
            if let Some(buffer_id) = state.find_editor_buffer_by_path(&path) {
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
                        if state.editor_buffer().is_dirty() {
                            state.buffers.push(buf);
                            state.active_editor_buffer_id = state.buffers.len() - 1;
                        } else {
                            state.buffers[state.active_editor_buffer_id] = buf;
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
            with_focused_buffer(state, |buf| buf.delete_to_end());
        }
        Action::ChangeToEnd => {
            // ChangeToEnd swaps to Insert mode unconditionally, mirroring
            // vim's `C` semantics (no-op buffer + still-in-Insert is the
            // documented behaviour when there's no focused buffer).
            with_focused_buffer(state, |buf| buf.delete_to_end());
            state.mode = Mode::Insert;
        }
        Action::SubstituteChar => {
            with_focused_buffer(state, |buf| buf.delete_forward());
            state.mode = Mode::Insert;
        }
        Action::SubstituteLine => {
            with_focused_buffer(state, |buf| buf.substitute_line());
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
                state.editor_search.term = Some(term.clone());
                state.editor_search.whole_word = true;
                state.editor_search.forward = true;
                if let Some(buf) = state.focused_buffer_mut() {
                    buf.search_next_match(&term, true);
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
                state.editor_search.term = Some(term.clone());
                state.editor_search.whole_word = true;
                state.editor_search.forward = false;
                if let Some(buf) = state.focused_buffer_mut() {
                    buf.search_prev_match(&term, true);
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
                if let Some(buf) = state.focused_buffer_mut() {
                    let found = if forward {
                        buf.search_next_match(&term, whole_word)
                    } else {
                        buf.search_prev_match(&term, whole_word)
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
                if let Some(buf) = state.focused_buffer_mut() {
                    let found = if forward {
                        buf.search_prev_match(&term, whole_word)
                    } else {
                        buf.search_next_match(&term, whole_word)
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
            state.search_prompt = Some(SearchPrompt::new());
        }
        Action::SearchPromptCancel => {
            state.search_prompt = None;
        }
        Action::SearchPromptBackspace => {
            if let Some(p) = state.search_prompt.as_mut() {
                p.backspace();
            }
        }
        Action::SearchPromptInput(ch) => {
            if let Some(p) = state.search_prompt.as_mut() {
                p.insert(ch);
            }
        }
        Action::SearchPromptExecute => {
            if let Some(prompt) = state.search_prompt.take() {
                let term = prompt.input;
                if !term.is_empty() {
                    state.editor_search.term = Some(term.clone());
                    state.editor_search.whole_word = false;
                    state.editor_search.forward = true;
                    if let Some(buf) = state.focused_buffer_mut() {
                        let found = buf.search_next_match(&term, false);
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
    }

    if !preserve_count {
        state.pending_count = None;
    }

    ActionOutcome {
        should_exit,
        state_changed: changed,
    }
}
