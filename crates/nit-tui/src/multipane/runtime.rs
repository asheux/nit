use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use nit_core::{AgentChannel, AgentMessage, AppState, UiSelection, UiSelectionPane};
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Terminal,
};

use super::dir_search::{self, ParsedQuery};
use super::dir_search_runner::{DirSearchEvent, DirSearchRunner};
use super::dispatch::with_pane_aliased;
use super::focus;
use super::grid;
use super::persistence;
use super::roster_view;
use super::selection;
use super::setup::materialise_pane_lane;
use crate::app::{
    chat_history_next, chat_history_prev, clear_chat_esc_state, handle_abort,
    handle_chat_input_editing_key, is_global_quit_key, lane_has_in_flight_turn,
    parse_abort_command, record_chat_esc_press, submit_chat_input_and_dispatch, AbortScope,
};
use crate::claude_runner::{ClaudeRunner, ClaudeRunnerConfig};
use crate::codex_runner::{CodexRunner, CodexRunnerConfig, CodexRuntimeMode};
use crate::shadow::ShadowRuntime;
use crate::swarm::{SwarmRuntime, SYSTEM_ALERT_KIND};
use crate::theme::Theme;
use crate::vitals::VitalsState;
use crate::widgets::agent_console_view::{self, ChatCursor};
use crate::widgets::artifacts_popup;

const TICK_RATE: Duration = Duration::from_millis(50);

pub fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut AppState,
    theme: &Theme,
    log_rx: Receiver<String>,
    codex_runtime: CodexRuntimeMode,
    codex_config: CodexRunnerConfig,
    claude_config: ClaudeRunnerConfig,
) -> io::Result<()> {
    state.agents.codex_max_parallel_turns = codex_config.max_parallel_turns;
    state.agents.claude_max_parallel_turns = claude_config.max_parallel_turns;
    if state.gitignored_dirs.is_empty() {
        state.gitignored_dirs = crate::file_watcher::parse_gitignore_dirs(&state.workspace_root);
    }
    let codex = CodexRunner::spawn(codex_runtime, codex_config, None);
    let claude = ClaudeRunner::spawn(claude_config);
    let dir_search_runner = DirSearchRunner::spawn();
    let mut swarm = SwarmRuntime::default();
    let mut shadow = ShadowRuntime::default();
    let mut vitals = VitalsState::default();
    clear_chat_esc_state();
    let mut clipboard: Option<arboard::Clipboard> = arboard::Clipboard::new().ok();

    let workspace_root = state.workspace_root.clone();
    let had_prior_session = persistence::session_path(&workspace_root)
        .map(|p| p.exists())
        .unwrap_or(false);
    if let Some(prior) = persistence::load_session(&workspace_root) {
        if let Some(mp) = state.multipane.as_mut() {
            // Best-effort merge: drop the prior layout if pane count
            // changed (different --panes flag). Drift in the roster is
            // handled later by validating selected_agent_id against the
            // live agents list.
            let _ = persistence::merge_prior(mp, prior);
        }
    }
    let mut last_save = Instant::now();
    let mut last_focused = state.multipane.as_ref().map(|mp| mp.focused);
    let save_debounce = Duration::from_secs(1);

    loop {
        // Drain any already-buffered input BEFORE the agent-bus drain so
        // wheel / PgUp / Ctrl+Q events don't get queued behind a 100+ event
        // swarm burst. Non-blocking drain of pending events (capped at 32 per
        // pass) so a runaway producer can't starve the bus. The bottom-of-loop
        // `event::poll(TICK_RATE)` remains the idle-wait pump.
        if event::poll(Duration::from_millis(0))? {
            let area = terminal_size(terminal)?;
            let should_quit = drain_input_burst(
                state,
                &mut vitals,
                &codex,
                &claude,
                &mut swarm,
                &mut shadow,
                &dir_search_runner,
                &mut clipboard,
                theme,
                area,
            )?;
            if should_quit {
                finalize_session(state, &workspace_root, had_prior_session);
                return Ok(());
            }
        }
        for log_line in log_rx.try_iter() {
            // v1: discard. Phase 5 may route to a status row.
            let _ = log_line;
        }
        // Mirror the single-pane runner's per-event pipeline. Without
        // this, swarm follow-ups never dispatch (planner finishes,
        // propose/judge/review never run), breathers stick on
        // "Waiting…", and queued turns never drain. Genome retries are
        // disabled in multipane v1 (genome_worker = None).
        for event in codex.events.try_iter() {
            crate::app::event_drain::drain_codex_event(
                state,
                &mut vitals,
                &codex,
                &claude,
                &mut swarm,
                &mut shadow,
                None,
                event,
            );
        }
        for event in claude.events.try_iter() {
            crate::app::event_drain::drain_claude_event(
                state,
                &mut vitals,
                &codex,
                &claude,
                &mut swarm,
                &mut shadow,
                None,
                event,
            );
        }
        for event in dir_search_runner.events.try_iter() {
            apply_dir_search_event(state, event);
        }

        // Poll background genome work the same way the single-pane
        // runner does (`app/runner.rs`). Without these, the per-pane
        // swarm runs spawn their genome gate / review worker threads,
        // the workers complete and post their result on an mpsc, and
        // nobody ever reads the channel — so the breather sticks at
        // "Verifying (genome gate) ..." forever and the verifier never
        // dispatches. Each poll returns ready dispatches; we send them
        // through the same `apply_swarm_task_role` + `dispatch_agent_prompt`
        // pipeline `app/runner.rs` uses.
        for mut dispatch in swarm.poll_genome_gates(state) {
            crate::app::augment_dispatch_prompt_with_landscape(state, &swarm, &mut dispatch);
            crate::app::apply_swarm_task_role(state, &dispatch);
            crate::app::dispatch_agent_prompt(
                state,
                &mut vitals,
                Some(&codex),
                Some(&claude),
                dispatch.agent_id,
                Some(dispatch.mission_id),
                dispatch.prompt,
            );
        }
        for mut dispatch in swarm.poll_genome_reviews(state) {
            crate::app::augment_dispatch_prompt_with_landscape(state, &swarm, &mut dispatch);
            crate::app::apply_swarm_task_role(state, &dispatch);
            crate::app::dispatch_agent_prompt(
                state,
                &mut vitals,
                Some(&codex),
                Some(&claude),
                dispatch.agent_id,
                Some(dispatch.mission_id),
                dispatch.prompt,
            );
        }

        capture_pane_mission_ids(state);

        // Animate the breather. The single-pane runner ticks
        // `frame_count` in `app/draw.rs:408`; multipane has its own draw
        // path so we have to do it explicitly. Without this, the histogram
        // glyph next to "Working ..." / "Verifying ..." stays frozen on a
        // single frame regardless of how long an agent runs.
        state.metrics.frame_count = state.metrics.frame_count.wrapping_add(1);

        terminal.draw(|frame| {
            let area = frame.size();
            let cursor = render_grid(frame, area, state, &swarm, theme);
            if state.agents.artifacts_popup_open {
                let popup_area = popup_rect_for(area, artifacts_popup::preferred_size(area));
                artifacts_popup::render(frame, popup_area, state, &swarm, theme);
            }
            if let Some(c) = cursor {
                frame.set_cursor(c.x, c.y);
            }
        })?;
        // Match the single-pane chat-input caret shape so the operator
        // sees the same thin steady bar across both modes; without this,
        // multipane inherits whatever the terminal's default is (usually
        // a wide block) and the caret looks fat / inconsistent. The bar
        // is "steady" — the visible blink comes from gating
        // `frame.set_cursor` on `cursor_visible(state)` (a frame-counter
        // pulse), exactly like single-pane.
        let _ = crossterm::execute!(
            terminal.backend_mut(),
            crossterm::cursor::SetCursorStyle::SteadyBar
        );

        if !event::poll(TICK_RATE)? {
            continue;
        }
        let area = terminal_size(terminal)?;
        // Coalesce a burst of input (e.g. wheel scroll fires 5–20 events
        // per gesture) so we render once per batch instead of once per
        // event. Bounded at 32 so a runaway producer can't starve the
        // redraw indefinitely.
        let should_quit = drain_input_burst(
            state,
            &mut vitals,
            &codex,
            &claude,
            &mut swarm,
            &mut shadow,
            &dir_search_runner,
            &mut clipboard,
            theme,
            area,
        )?;
        if should_quit {
            finalize_session(state, &workspace_root, had_prior_session);
            return Ok(());
        }

        // Debounced session save: focus change or cwd change is the
        // signal; an idle pane never costs disk IO.
        let current_focus = state.multipane.as_ref().map(|mp| mp.focused);
        let focus_changed = current_focus != last_focused;
        if focus_changed {
            last_focused = current_focus;
        }
        if focus_changed && last_save.elapsed() >= save_debounce {
            if let Some(mp) = state.multipane.as_ref() {
                let _ = persistence::save_session(mp, &workspace_root);
            }
            last_save = Instant::now();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn drain_input_burst(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
    dir_runner: &DirSearchRunner,
    clipboard: &mut Option<arboard::Clipboard>,
    theme: &Theme,
    area: Rect,
) -> io::Result<bool> {
    for _ in 0..32 {
        match event::read()? {
            // Accept both Press and Repeat so held keys (Backspace,
            // Delete, arrow nav, character repeats) auto-fire — single
            // pane already does this in `app/runner.rs` and the UX
            // mismatch was breaking long edits in multipane.
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if handle_key(
                    state, vitals, codex, claude, swarm, shadow, dir_runner, key, clipboard, area,
                ) {
                    return Ok(true);
                }
            }
            // Bracketed paste arrives as a single text blob (not a
            // sequence of Char key events), so without this branch
            // Cmd-V / right-click-paste / iTerm paste in multipane is
            // silently dropped.
            Event::Paste(text) => handle_paste(state, &text),
            Event::Mouse(mouse) => handle_mouse(state, swarm, theme, clipboard, area, mouse),
            _ => {}
        }
        if !event::poll(Duration::from_millis(0))? {
            break;
        }
    }
    Ok(false)
}

/// Routes a bracketed-paste blob to whichever input is currently
/// receiving keystrokes: the artifacts popup chat input when it's
/// open, otherwise the focused pane's chat input. Mirrors single-pane
/// `handle_paste_event` for the two surfaces multipane exposes — the
/// editor / fuzzy-search / command-line paths from single-pane don't
/// apply because multipane has no editor focus.
fn handle_paste(state: &mut AppState, text: &str) {
    if text.is_empty() {
        return;
    }
    if state.agents.artifacts_popup_open {
        let _ = crate::app::insert_popup_chat_text(state, text);
        return;
    }
    let pane_idx = focused_pane_idx(state);
    with_pane_aliased(state, pane_idx, |state| {
        let _ = crate::app::insert_chat_input_text(state, text);
    });
}

// Discard the session file if nothing was run yet and no prior file existed;
// otherwise persist so the operator can resume.
fn finalize_session(state: &AppState, workspace_root: &Path, had_prior: bool) {
    let Some(mp) = state.multipane.as_ref() else {
        return;
    };
    if persistence::is_fresh(mp) && !had_prior {
        persistence::drop_session(workspace_root);
        return;
    }
    let _ = persistence::save_session(mp, workspace_root);
}

fn terminal_size(terminal: &Terminal<CrosstermBackend<Stdout>>) -> io::Result<Rect> {
    let size = terminal.size()?;
    Ok(Rect::new(0, 0, size.width, size.height))
}

/// Centered popup rect, matching `app::layout_rects::dynamic_popup_rect`
/// (which is `pub(super)`). Local copy keeps the multipane crate
/// independent of the single-pane app module's private layout helpers.
fn popup_rect_for(screen: Rect, desired: (u16, u16)) -> Rect {
    use ratatui::layout::{Constraint, Direction, Layout};
    let max_w = screen.width.saturating_sub(4).max(10);
    let max_h = screen.height.saturating_sub(2).max(5);
    let width = desired.0.min(max_w);
    let height = desired.1.min(max_h);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((screen.height.saturating_sub(height)) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(screen)[1];
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((screen.width.saturating_sub(width)) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(vertical)[1]
}

fn capture_pane_mission_ids(state: &mut AppState) {
    // pre-collect to avoid two simultaneous borrows on state inside the loop
    let real_mission_ids: std::collections::HashSet<String> =
        state.agents.missions.iter().map(|m| m.id.clone()).collect();
    let lane_missions: std::collections::HashMap<String, Option<String>> = state
        .agents
        .agents
        .iter()
        .map(|l| (l.id.clone(), l.current_mission.clone()))
        .collect();
    let Some(mp) = state.multipane.as_mut() else {
        return;
    };
    for pane in &mut mp.panes {
        // Only short-circuit when the pane already has a *real* swarm
        // mission overlaid; the synthetic chat id (or a stale id no
        // longer in `state.agents.missions`) must not block capture.
        let already_real = pane
            .mission_id
            .as_deref()
            .is_some_and(|m| real_mission_ids.contains(m));
        if already_real {
            if let Some(mid) = pane.mission_id.clone() {
                if !pane.mission_ids.iter().any(|m| m == &mid) {
                    pane.mission_ids.push(mid);
                }
            }
            continue;
        }
        let lookup_id = Some(pane.agent_id.as_str())
            .filter(|s| !s.is_empty())
            .or(pane.selected_agent_id.as_deref());
        let Some(lookup_id) = lookup_id else { continue };
        let candidate = lane_missions.get(lookup_id).cloned().unwrap_or(None);
        if let Some(mid) = candidate {
            pane.mission_id = Some(mid.clone());
            if !pane.mission_ids.iter().any(|m| m == &mid) {
                pane.mission_ids.push(mid);
            }
        }
    }
}

const MIN_PANE_WIDTH: u16 = 20;
const MIN_PANE_HEIGHT: u16 = 10;
const BOTTOM_HINT: &str = "MULTIPANE  ·  Tab cycle  ·  Ctrl+Q quit  ·  F1 help";

fn render_grid(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &mut AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
) -> Option<ChatCursor> {
    let (panes_len, focused_idx, cols, rows) = {
        let mp = state.multipane.as_ref()?;
        (mp.panes.len(), mp.focused, mp.grid_cols, mp.grid_rows)
    };
    if panes_len == 0 {
        return None;
    }

    // Reserve 1 row each for the top status strip and bottom indicator
    // when the terminal is tall enough; otherwise the panes get the
    // full area and chrome is suppressed.
    let chrome = area.height >= 4;
    let grid_area = if chrome {
        Rect::new(area.x, area.y + 1, area.width, area.height - 2)
    } else {
        area
    };
    if chrome {
        let top = Rect::new(area.x, area.y, area.width, 1);
        let bottom = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
        paint_top_strip(frame, top, state, theme);
        paint_bar(
            frame,
            bottom,
            BOTTOM_HINT.to_string(),
            bottom_strip_style(theme),
        );
    }

    let too_small = grid_too_small(grid_area, cols, rows);
    if too_small {
        let msg = format!(
            "Terminal too small for {panes_len} panes — resize or relaunch with --panes <smaller>"
        );
        paint_bar(frame, grid_area, msg, top_strip_style(theme));
        return None;
    }

    let mut cursor: Option<ChatCursor> = None;
    for idx in 0..panes_len {
        let focused = idx == focused_idx;
        if let Some(c) = render_one_pane(
            frame, grid_area, state, swarm, theme, idx, cols, rows, focused,
        ) {
            if focused {
                cursor = Some(c);
            }
        }
    }

    let help_open = matches!(state.multipane.as_ref(), Some(mp) if mp.help_open);
    if help_open {
        render_help_overlay(frame, popup_rect_for(area, (60, 18)), theme);
        // Hide the cursor while the overlay covers the input.
        return None;
    }
    cursor
}

fn paint_bar(frame: &mut ratatui::Frame, rect: Rect, text: String, style: Style) {
    if rect.height == 0 || rect.width == 0 {
        return;
    }
    // `.style(style)` paints the entire rect with the bar's bg, so cells
    // beyond the text length still inherit the strip background. Without
    // it, `Span::styled` only colours the text cells and the bar appears
    // truncated on terminals wider than the label.
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text, style))).style(style),
        rect,
    );
}

fn top_strip_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.title_focused)
        .bg(theme.background)
        .add_modifier(Modifier::BOLD)
}

fn bottom_strip_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.border)
        .bg(theme.background)
        .add_modifier(Modifier::DIM)
}

fn grid_too_small(area: Rect, cols: usize, rows: usize) -> bool {
    if cols == 0 || rows == 0 {
        return false;
    }
    let pane_w = area.width / cols as u16;
    let pane_h = area.height / rows as u16;
    pane_w < MIN_PANE_WIDTH || pane_h < MIN_PANE_HEIGHT
}

fn paint_top_strip(frame: &mut ratatui::Frame, rect: Rect, state: &AppState, theme: &Theme) {
    let Some(mp) = state.multipane.as_ref() else {
        return;
    };
    let cwd = mp
        .panes
        .get(mp.focused)
        .map(|p| pane_path_label(state, &p.cwd))
        .unwrap_or_default();
    let mut label = format!(
        "MULTIPANE  pane {}/{}  cwd={cwd}",
        mp.focused + 1,
        mp.panes.len()
    );
    if let Some(status) = state.status.as_deref() {
        if !status.is_empty() {
            label.push_str("  STATUS:");
            label.push_str(status);
        }
    }
    paint_bar(frame, rect, label, top_strip_style(theme));
}

fn render_help_overlay(frame: &mut ratatui::Frame, rect: Rect, theme: &Theme) {
    let lines = [
        "Multipane keymap",
        "",
        "  Tab            cycle pane focus forward",
        "  Shift+Tab      cycle pane focus backward",
        "  Ctrl+R         revert focused pane to roster",
        "  Ctrl+/  / F2   toggle dir-search overlay",
        "  Enter          submit prompt / commit roster pick",
        "  Up / Down      walk per-pane chat history",
        "  PgUp / PgDn    scroll pane chat thread",
        "  Ctrl+C         abort focused pane (empty input)",
        "  Esc Esc        abort focused pane",
        "  Ctrl+Q         quit multipane",
        "  F1 / ?         toggle this overlay",
        "",
        "  In chat: @swarm @shadow @all @new @queue @q",
        "           /abort  /abort all  /abort <agent-id>",
    ];
    let body: Vec<Line<'static>> = lines
        .iter()
        .map(|l| {
            Line::from(Span::styled(
                (*l).to_string(),
                Style::default().fg(theme.foreground),
            ))
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_focused))
        .style(Style::default().bg(theme.background))
        .title(Span::styled(
            " HELP — multipane (F1 / ? / Esc to close) ",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    frame.render_widget(ratatui::widgets::Clear, rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(body), inner);
}

#[allow(clippy::too_many_arguments)]
fn render_one_pane(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &mut AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
    idx: usize,
    cols: usize,
    rows: usize,
    focused: bool,
) -> Option<ChatCursor> {
    let rect = grid::pane_rect(area, cols, rows, idx);
    if rect.width < 2 || rect.height < 2 {
        return None;
    }
    let inner = paint_pane_chrome(frame, rect, state, idx, focused, theme);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    sync_dir_search_viewport(state, idx, inner.height);
    let pane = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(idx))
        .cloned()?;

    if pane.selected_agent_id.is_none() && pane.agent_id.is_empty() {
        let backend_filter = state
            .multipane
            .as_ref()
            .and_then(|mp| mp.backend_filter.clone());
        clamp_roster_scroll(state, idx, inner.height as usize);
        let updated_pane = state
            .multipane
            .as_ref()
            .and_then(|mp| mp.panes.get(idx))
            .cloned()
            .unwrap_or(pane);
        let body_rect = render_pane_dir_search_overlay(frame, inner, &updated_pane, theme);
        roster_view::render(
            frame,
            body_rect,
            state,
            &updated_pane,
            backend_filter.as_deref(),
            focused,
            theme,
        );
        return None;
    }

    let body_rect = render_pane_dir_search_overlay(frame, inner, &pane, theme);
    // Alias `state.agents.selected_*` to this pane's agent / mission for
    // the duration of the render so `breather_rows_for_user_prompt` and
    // `inline_breather_rows` (which read `selected_context_*`) only show
    // this pane's lanes. Restored before the next pane renders to avoid
    // bleed.
    let saved_agent = state.agents.selected_agent.clone();
    let saved_mission = state.agents.selected_mission.clone();
    let saved_mission_selected = state.agents.mission_selected;
    let pane_agent_id = if !pane.agent_id.is_empty() {
        Some(pane.agent_id.clone())
    } else {
        pane.selected_agent_id.clone()
    };
    state.agents.selected_agent = pane_agent_id;
    // For default-chat (no real swarm overlay), fall back to the pane's
    // synthetic chat id so `breather_rows_for_user_prompt` sees a non-None
    // `mission_ctx` and partitions other panes' agents OUT of `primary_ids`.
    // Mirrors the alias source in `dispatch::with_pane_aliased`.
    state.agents.selected_mission = pane
        .mission_id
        .clone()
        .or_else(|| (!pane.chat_mission_id.is_empty()).then(|| pane.chat_mission_id.clone()));
    // Mirror `with_pane_aliased`: disable the global mission fallback during this pane's render.
    state.agents.mission_selected = usize::MAX;
    let cursor = agent_console_view::render_pane(
        frame,
        body_rect,
        state,
        Some(swarm),
        theme,
        &pane,
        focused,
    );
    state.agents.selected_agent = saved_agent;
    state.agents.selected_mission = saved_mission;
    state.agents.mission_selected = saved_mission_selected;
    cursor
}

/// Stash the current visible-row count on `DirSearchState.last_visible`
/// (so key handlers can clamp the viewport without the layout rect) and
/// clamp `view_offset` to keep the highlight in view.
fn sync_dir_search_viewport(state: &mut AppState, idx: usize, inner_height: u16) {
    let Some(pane) = pane_at_mut(state, idx) else {
        return;
    };
    let Some(ds) = pane.dir_search.as_mut() else {
        return;
    };
    let visible = compute_dropdown_rows(inner_height, ds.results.len());
    ds.last_visible = visible;
    clamp_viewport(ds, visible as usize);
}

/// Walk the highlight back into the visible window after a results
/// refresh, a Down/Up key, or a viewport-height change. Idempotent.
fn clamp_viewport(ds: &mut nit_core::DirSearchState, visible: usize) {
    let len = ds.results.len();
    if len == 0 {
        ds.view_offset = 0;
        ds.selected = 0;
        return;
    }
    if ds.selected >= len {
        ds.selected = len - 1;
    }
    let visible = visible.max(1);
    let max_offset = len.saturating_sub(visible);
    if ds.selected < ds.view_offset {
        ds.view_offset = ds.selected;
    }
    if ds.selected >= ds.view_offset + visible {
        ds.view_offset = ds.selected + 1 - visible;
    }
    if ds.view_offset > max_offset {
        ds.view_offset = max_offset;
    }
}

const DIR_SEARCH_DROPDOWN_MIN_ROWS: u16 = 3;
const DIR_SEARCH_DROPDOWN_MAX_ROWS: u16 = 16;
const DIR_SEARCH_INPUT_ROWS: u16 = 1;

fn compute_dropdown_rows(inner_height: u16, results_len: usize) -> u16 {
    let budget = inner_height.saturating_sub(DIR_SEARCH_INPUT_ROWS) / 2;
    let clamped = budget.clamp(DIR_SEARCH_DROPDOWN_MIN_ROWS, DIR_SEARCH_DROPDOWN_MAX_ROWS);
    clamped.min(results_len.max(1) as u16)
}

fn compute_dir_search_layout(
    inner: Rect,
    pane: &nit_core::PaneSession,
) -> Option<(u16, Rect, Rect)> {
    let ds = pane.dir_search.as_ref()?;
    if inner.height < DIR_SEARCH_INPUT_ROWS + DIR_SEARCH_DROPDOWN_MIN_ROWS {
        return None;
    }
    let visible_rows = compute_dropdown_rows(inner.height, ds.results.len());
    let bar_rect = Rect::new(inner.x, inner.y, inner.width, DIR_SEARCH_INPUT_ROWS);
    let drop_rect = Rect::new(
        inner.x,
        inner.y + DIR_SEARCH_INPUT_ROWS,
        inner.width,
        visible_rows,
    );
    Some((visible_rows, bar_rect, drop_rect))
}

/// Body rect for a pane's content area, taking into account whether the
/// dir-search overlay is open. Mirrors `render_pane_dir_search_overlay`'s
/// return value exactly so click hit-tests resolve to the same rows the
/// renderer painted. Single source of truth — both call sites must use
/// it or roster clicks misroute when the dropdown is open.
fn dir_search_body_rect(inner: Rect, pane: &nit_core::PaneSession) -> Rect {
    let Some((visible_rows, _, _)) = compute_dir_search_layout(inner, pane) else {
        return inner;
    };
    let total = DIR_SEARCH_INPUT_ROWS + visible_rows;
    Rect::new(
        inner.x,
        inner.y + total,
        inner.width,
        inner.height.saturating_sub(total),
    )
}

fn render_pane_dir_search_overlay(
    frame: &mut ratatui::Frame,
    inner: Rect,
    pane: &nit_core::PaneSession,
    theme: &Theme,
) -> Rect {
    let Some((visible_rows, bar_rect, drop_rect)) = compute_dir_search_layout(inner, pane) else {
        return inner;
    };
    let ds = pane.dir_search.as_ref().unwrap();
    let bar_text = format!(
        " search: {}{} ",
        ds.query,
        if ds.show_hidden { "  [hidden]" } else { "" }
    );
    let bar_style = Style::default()
        .fg(theme.title_focused)
        .bg(theme.background)
        .add_modifier(Modifier::BOLD);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(bar_text, bar_style))),
        bar_rect,
    );
    let visible_usize = visible_rows as usize;
    let end = ds
        .view_offset
        .saturating_add(visible_usize)
        .min(ds.results.len());
    let slice_start = ds.view_offset.min(end);
    let lines: Vec<Line<'static>> = ds.results[slice_start..end]
        .iter()
        .enumerate()
        .map(|(rel_idx, path)| {
            let abs_idx = slice_start + rel_idx;
            let label = breadcrumb_label(&ds.base, path);
            let depth = path
                .strip_prefix(&ds.base)
                .ok()
                .map(|rel| rel.components().count().saturating_sub(1))
                .unwrap_or(0);
            let indent = "  ".repeat(depth);
            let style = if abs_idx == ds.selected {
                Style::default()
                    .fg(theme.background)
                    .bg(theme.border_focused)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground)
            };
            Line::from(Span::styled(format!(" {indent}{label} "), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), drop_rect);
    dir_search_body_rect(inner, pane)
}

/// Render a path as `parent/child/.../leaf/` relative to `base`. Uses
/// `Path::components()` joined with literal `/` so cross-platform output
/// stays stable; falls back to the absolute path display when the entry
/// isn't actually under base (e.g. symlink hop).
fn breadcrumb_label(base: &Path, path: &Path) -> String {
    let Ok(rel) = path.strip_prefix(base) else {
        return path.display().to_string();
    };
    let joined: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if joined.is_empty() {
        return path
            .file_name()
            .map(|n| format!("{}/", n.to_string_lossy()))
            .unwrap_or_else(|| path.display().to_string());
    }
    format!("{}/", joined.join("/"))
}

fn clamp_roster_scroll(state: &mut AppState, pane_idx: usize, height: usize) {
    let backend_filter = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.backend_filter.clone());
    let Some(pane_clone) = pane_at(state, pane_idx).cloned() else {
        return;
    };
    let rows = roster_view::compute_rows(state, &pane_clone, backend_filter.as_deref());
    let max_scroll = rows.len().saturating_sub(height);
    let stops = roster_view::selectable_count(&rows);
    let Some(pane) = pane_at_mut(state, pane_idx) else {
        return;
    };
    pane.roster_scroll = pane.roster_scroll.min(max_scroll);
    pane.roster_cursor = if stops == 0 {
        0
    } else {
        pane.roster_cursor.min(stops - 1)
    };
}

fn pane_at(state: &AppState, pane_idx: usize) -> Option<&nit_core::PaneSession> {
    state.multipane.as_ref()?.panes.get(pane_idx)
}

fn pane_at_mut(state: &mut AppState, pane_idx: usize) -> Option<&mut nit_core::PaneSession> {
    state.multipane.as_mut()?.panes.get_mut(pane_idx)
}

fn paint_pane_chrome(
    frame: &mut ratatui::Frame,
    rect: Rect,
    state: &AppState,
    idx: usize,
    focused: bool,
    theme: &Theme,
) -> Rect {
    let mp = match state.multipane.as_ref() {
        Some(mp) => mp,
        None => return rect,
    };
    let Some(pane) = mp.panes.get(idx) else {
        return rect;
    };
    let cwd_text = pane_path_label(state, &pane.cwd);
    let mode_label = if pane.selected_agent_id.is_none() && pane.agent_id.is_empty() {
        "roster"
    } else {
        "chat"
    };
    let title = format!(" pane {idx} · {mode_label} · {cwd_text} ");
    let border_color = if focused {
        theme.border_focused
    } else {
        theme.border
    };
    let title_color = if focused {
        theme.title_focused
    } else {
        theme.title
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(if focused {
            BorderType::Thick
        } else {
            BorderType::Plain
        })
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            title,
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(theme.background));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    paint_hint_line(
        frame,
        inner,
        pane_in_roster_mode(state, idx),
        pane_dir_search_active(state, idx),
        theme,
    );
    inner_rect_after_hint(inner)
}

fn pane_dir_search_active(state: &AppState, idx: usize) -> bool {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(idx))
        .map(|p| p.dir_search.is_some())
        .unwrap_or(false)
}

fn pane_path_label(state: &AppState, cwd: &Path) -> String {
    let workspace_root = state.workspace_root.as_path();
    if let Ok(rel) = cwd.strip_prefix(workspace_root) {
        let rel_str = rel.to_string_lossy();
        let project = workspace_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        return match (project, rel_str.is_empty()) {
            (Some(name), true) => name,
            (Some(name), false) => format!("{name}/{rel_str}"),
            (None, false) => rel_str.into_owned(),
            (None, true) => cwd.display().to_string(),
        };
    }
    let Some(home) = std::env::var_os("HOME") else {
        return cwd.display().to_string();
    };
    let home_path = std::path::PathBuf::from(home);
    let Ok(rel) = cwd.strip_prefix(&home_path) else {
        return cwd.display().to_string();
    };
    let rel_str = rel.to_string_lossy();
    if rel_str.is_empty() {
        "~".into()
    } else {
        format!("~/{rel_str}")
    }
}

fn pane_in_roster_mode(state: &AppState, idx: usize) -> bool {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(idx))
        .map(|p| p.selected_agent_id.is_none() && p.agent_id.is_empty())
        .unwrap_or(false)
}

fn paint_hint_line(
    frame: &mut ratatui::Frame,
    inner: Rect,
    in_roster: bool,
    in_dir_search: bool,
    theme: &Theme,
) {
    if inner.height == 0 {
        return;
    }
    let hint_text = if in_dir_search {
        " ↑/↓ Ctrl+J/K · ←/→ Ctrl+H/L expand · Enter cd · Alt+F hidden · Esc close "
    } else if in_roster {
        " ↑/↓ j/k · h/l fold · Space check · Enter commit · Tab pane "
    } else {
        " /abort · Ctrl+C · Esc Esc · Ctrl+R roster · PgUp/PgDn scroll "
    };
    let hint = Line::from(Span::styled(
        hint_text,
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::ITALIC | Modifier::DIM),
    ));
    let hint_rect = Rect::new(inner.x, inner.y, inner.width, 1);
    frame.render_widget(Paragraph::new(hint), hint_rect);
}

fn inner_rect_after_hint(inner: Rect) -> Rect {
    if inner.height <= 1 {
        return Rect::new(inner.x, inner.y, inner.width, 0);
    }
    Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1)
}

/// Handle a key event. Returns `true` to exit the loop.
#[allow(clippy::too_many_arguments)]
fn handle_key(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
    dir_runner: &DirSearchRunner,
    key: KeyEvent,
    clipboard: &mut Option<arboard::Clipboard>,
    area: Rect,
) -> bool {
    if let Some(exit) = consume_global_chrome_keys(state, &key) {
        return exit;
    }

    if !matches!(key.code, KeyCode::Esc) {
        clear_chat_esc_state();
    }

    if try_cycle_focus(state, &key) {
        return false;
    }

    if focused_pane_dir_search_active(state) {
        return handle_dir_search_key(state, dir_runner, key, codex, claude, swarm, shadow);
    }

    if focused_pane_in_roster_mode(state) {
        return handle_roster_key(state, codex, claude, swarm, key);
    }
    handle_chat_key(
        state, vitals, codex, claude, swarm, shadow, dir_runner, key, clipboard, area,
    )
}

/// Resolve the chrome short-circuits — artifacts popup, help overlay
/// toggle, Ctrl+Q. Returns:
/// - `Some(true)`  → exit the run loop (Ctrl+Q).
/// - `Some(false)` → key was consumed by a chrome short-circuit, do
///   nothing else this tick.
/// - `None`        → key is for the active pane; continue dispatch.
fn consume_global_chrome_keys(state: &mut AppState, key: &KeyEvent) -> Option<bool> {
    if state.agents.artifacts_popup_open && matches!(key.code, KeyCode::Esc) {
        state.agents.artifacts_popup_open = false;
        clear_chat_esc_state();
        return Some(false);
    }

    if matches!(state.multipane.as_ref(), Some(mp) if mp.help_open) {
        let close = matches!(key.code, KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('?'));
        if close {
            set_help_open(state, false);
            clear_chat_esc_state();
        }
        return Some(false);
    }

    if is_global_quit_key(key) {
        return Some(true);
    }

    let chord = key.modifiers;
    let is_unmodified_question_mark = matches!(key.code, KeyCode::Char('?'))
        && !chord.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let opens_help = matches!(key.code, KeyCode::F(1))
        || (is_unmodified_question_mark && focused_chat_input_is_empty(state));
    if opens_help {
        set_help_open(state, true);
        return Some(false);
    }
    None
}

fn set_help_open(state: &mut AppState, open: bool) {
    if let Some(mp) = state.multipane.as_mut() {
        mp.help_open = open;
    }
}

/// Tab / Shift+Tab / BackTab cycle pane focus regardless of mode and
/// never move the per-pane roster cursor. Closing dir-search on tab
/// is the safe default — operator can re-open it in the new pane.
fn try_cycle_focus(state: &mut AppState, key: &KeyEvent) -> bool {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let forward = match key.code {
        KeyCode::Tab => !shift,
        KeyCode::BackTab => false,
        _ => return false,
    };
    close_focused_dir_search(state);
    if let Some(mp) = state.multipane.as_mut() {
        if forward {
            focus::cycle_forward(mp);
        } else {
            focus::cycle_backward(mp);
        }
    }
    true
}

fn handle_roster_key(
    state: &mut AppState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    key: KeyEvent,
) -> bool {
    let modifiers = key.modifiers;
    let is_ctrl = modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Char('c') if is_ctrl => {
            push_pane_system_message(state, "no agent selected — nothing to abort".into());
            false
        }
        KeyCode::Esc => {
            if record_chat_esc_press() {
                push_pane_system_message(state, "no agent selected — nothing to abort".into());
                clear_chat_esc_state();
            }
            false
        }
        KeyCode::Up | KeyCode::Char('k') => {
            move_roster_cursor(state, -1);
            false
        }
        KeyCode::Down | KeyCode::Char('j') => {
            move_roster_cursor(state, 1);
            false
        }
        KeyCode::PageUp => {
            move_roster_cursor(state, -ROSTER_PAGE_STEP);
            false
        }
        KeyCode::PageDown => {
            move_roster_cursor(state, ROSTER_PAGE_STEP);
            false
        }
        KeyCode::Char('g') => {
            jump_roster_cursor_to_top(state);
            false
        }
        KeyCode::Char('G') => {
            jump_roster_cursor_to_bottom(state);
            false
        }
        KeyCode::Left | KeyCode::Char('h') => {
            collapse_at_cursor(state);
            false
        }
        KeyCode::Right | KeyCode::Char('l') => {
            expand_at_cursor(state);
            false
        }
        KeyCode::Char(' ') => {
            toggle_size_at_cursor(state);
            false
        }
        KeyCode::Enter => {
            commit_roster_selection(state, codex, claude, swarm);
            false
        }
        _ => false,
    }
}

const ROSTER_PAGE_STEP: i32 = 8;

#[allow(clippy::too_many_arguments)]
fn handle_chat_key(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
    dir_runner: &DirSearchRunner,
    key: KeyEvent,
    clipboard: &mut Option<arboard::Clipboard>,
    area: Rect,
) -> bool {
    let modifiers = key.modifiers;
    let is_ctrl = modifiers.contains(KeyModifiers::CONTROL);
    let is_super = modifiers.contains(KeyModifiers::SUPER);

    match key.code {
        KeyCode::Char('/') if is_ctrl => {
            toggle_focused_pane_dir_search(state, dir_runner);
            false
        }
        KeyCode::F(2) => {
            // F2 fallback: many terminals (default macOS Terminal.app)
            // do not deliver Ctrl+/. Same payload either way.
            toggle_focused_pane_dir_search(state, dir_runner);
            false
        }
        KeyCode::Char('r') if is_ctrl => {
            revert_focused_pane_to_roster(state);
            false
        }
        KeyCode::Char('c') if is_super => {
            // Best-effort macOS Cmd+C: only fires on terminals that
            // forward SUPER (Kitty / WezTerm / iTerm with CSI-u).
            // Default Terminal.app does not deliver this.
            // If a chat-thread selection was consumed, we're done; otherwise
            // fall through to the canonical input editor so input-box
            // selections still copy.
            if !try_copy_focused_pane_selection(state, clipboard, area) {
                with_focused_pane_aliased(state, |state| {
                    let _ = handle_chat_input_editing_key(&key, state, clipboard);
                });
            }
            false
        }
        KeyCode::Char('c') if is_ctrl => {
            if try_copy_focused_pane_selection(state, clipboard, area) {
                return false;
            }
            // Empty input: Ctrl+C is the abort sentinel for the focused
            // pane. Non-empty input: defer to the canonical handler, which
            // copies an active input selection or clears the input — same
            // behavior as single-pane.
            if focused_chat_input_is_empty(state) {
                abort_focused_pane(state, codex, claude, swarm, shadow);
            } else {
                with_focused_pane_aliased(state, |state| {
                    let _ = handle_chat_input_editing_key(&key, state, clipboard);
                });
            }
            false
        }
        KeyCode::Esc => {
            if record_chat_esc_press() {
                abort_focused_pane(state, codex, claude, swarm, shadow);
                clear_chat_esc_state();
            }
            false
        }
        KeyCode::Enter => {
            submit_focused_pane_input(state, vitals, codex, claude, swarm, shadow);
            false
        }
        KeyCode::Up if !modifiers.contains(KeyModifiers::SHIFT) => {
            with_focused_pane_aliased(state, |state| {
                let _ = chat_history_prev(state);
            });
            false
        }
        KeyCode::Down if !modifiers.contains(KeyModifiers::SHIFT) => {
            with_focused_pane_aliased(state, |state| {
                let _ = chat_history_next(state);
            });
            false
        }
        KeyCode::PageUp => {
            scroll_chat_thread(state, swarm, area, -CHAT_THREAD_PAGE_STEP);
            false
        }
        KeyCode::PageDown => {
            scroll_chat_thread(state, swarm, area, CHAT_THREAD_PAGE_STEP);
            false
        }
        _ => {
            with_focused_pane_aliased(state, |state| {
                let _ = handle_chat_input_editing_key(&key, state, clipboard);
            });
            false
        }
    }
}

/// Lens-B-aliased Enter handler. Snaps the focused pane's
/// `chat_input` / `selected_agent` / `selected_mission` /
/// `chat_prompt_history*` / `swarm_default_*` into `state.agents.*`,
/// runs the canonical `submit_chat_input_and_dispatch` (which handles
/// `/abort`, `@swarm`, `@shadow`, `@all`, `@new`, `@queue`, `@q`, queueing,
/// broadcast, advisories, swarm-followup re-activation, shadow auto-enable,
/// and `push_chat_message`), then mirrors the resulting state back onto
/// the pane.
pub(crate) fn submit_focused_pane_input(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
) {
    let pane_idx = focused_pane_idx(state);
    let Some(pane) = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(pane_idx))
    else {
        return;
    };
    let chat_input = pane.chat_input.clone();
    if chat_input.trim().is_empty() {
        return;
    }
    let bound = !pane.agent_id.is_empty()
        || pane
            .selected_agent_id
            .as_deref()
            .is_some_and(|id| !id.is_empty());

    super::dispatch::bridge_pane_effort_to_runner_focused(state, pane_idx);

    // Roster mode: only `/abort` is meaningful — fall through to the
    // alias path so the operator gets parity with chat aborts. Anything
    // else clears the input and posts a "no agent selected" notice.
    if !bound && parse_abort_command(&chat_input).is_none() {
        clear_focused_pane_input(state);
        push_pane_system_message(
            state,
            "no agent selected — press Ctrl+R to choose one".into(),
        );
        return;
    }

    if bound {
        pin_pane_chat_mission_on_lane(state, pane_idx);
    }

    with_focused_pane_aliased(state, |state| {
        let _ =
            submit_chat_input_and_dispatch(state, vitals, Some(codex), Some(claude), swarm, shadow);
    });
    if let Some(pane) = focused_pane_mut(state) {
        pane.has_run_mission = true;
    }
    capture_pane_mission_ids(state);
}

/// Wrapper around `with_pane_aliased` for the focused pane. Single
/// call site for chat-input editing and history nav so we don't
/// duplicate the focus lookup at every keystroke.
fn with_focused_pane_aliased<R>(state: &mut AppState, body: impl FnOnce(&mut AppState) -> R) -> R {
    let pane_idx = focused_pane_idx(state);
    with_pane_aliased(state, pane_idx, body)
}

const CHAT_THREAD_PAGE_STEP: i32 = 8;

fn scroll_chat_thread(state: &mut AppState, swarm: &SwarmRuntime, area: Rect, delta: i32) {
    let Some(mp) = state.multipane.as_ref() else {
        return;
    };
    let focused_idx = mp.focused;
    let Some(pane) = mp.panes.get(focused_idx).cloned() else {
        return;
    };
    let max_scroll = focused_pane_chat_thread_max_scroll(state, swarm, &pane, area, focused_idx);
    if let Some(p) = focused_pane_mut(state) {
        // Resolve the "stick to bottom" sentinel before applying delta —
        // otherwise PgUp from the bottom jumps to row 0 instead of one
        // page above the bottom (sentinel `as i32` wraps to -1 and
        // `(-1 + delta).max(0) = 0`).
        let resolved = resolve_chat_scroll_sentinel(p.chat_thread_scroll, max_scroll);
        let next = (resolved as i32 + delta).max(0) as usize;
        // Only re-engage the "stick to bottom" sentinel when the operator
        // scrolled DOWN past the current bottom. PgUp / wheel-up must
        // never re-engage it — otherwise a transient max_scroll dip
        // (breather rows oscillating mid-swarm) silently consumes the
        // operator's scroll-up and the viewport feels stuck.
        p.chat_thread_scroll = if delta > 0 && next >= max_scroll {
            nit_core::CONSOLE_SCROLL_BOTTOM
        } else {
            next.min(max_scroll)
        };
    }
}

/// Translate the "follow bottom" sentinel into a concrete row offset
/// for arithmetic. Other values pass through. Used by both the
/// keyboard PgUp/PgDn path and the mouse wheel path so they stay in
/// lockstep with each other and with the `min(max_scroll)` clamp the
/// renderer applies.
fn resolve_chat_scroll_sentinel(scroll: usize, max_scroll: usize) -> usize {
    if scroll == nit_core::CONSOLE_SCROLL_BOTTOM {
        max_scroll
    } else {
        scroll.min(max_scroll)
    }
}

/// Maximum legal `chat_thread_scroll` for the given pane. Mirrors the
/// renderer's clamp at `agent_console_view::render_pane` so wheel /
/// PgUp / PgDn never pin the stored scroll beyond the rendered window
/// — which would otherwise force the operator to "drain" stale scroll
/// before any visible movement happens.
fn focused_pane_chat_thread_max_scroll(
    state: &AppState,
    swarm: &SwarmRuntime,
    pane: &nit_core::PaneSession,
    area: Rect,
    pane_idx: usize,
) -> usize {
    if pane.selected_agent_id.is_none() && pane.agent_id.is_empty() {
        return 0;
    }
    let Some(thread_area) = pane_thread_area_for_pane(state, area, pane_idx, pane) else {
        return 0;
    };
    let agent_id = if pane.agent_id.is_empty() {
        pane.selected_agent_id.as_deref()
    } else {
        Some(pane.agent_id.as_str())
    };
    let rows = agent_console_view::build_pane_thread_rows_with_breathers_for_pane(
        state,
        Some(swarm),
        Some(pane.pane_id),
        agent_id,
        pane.mission_id.as_deref().or_else(|| {
            (!pane.chat_mission_id.is_empty()).then_some(pane.chat_mission_id.as_str())
        }),
        thread_area.width.max(1) as usize,
        !pane.has_run_mission,
    );
    rows.len()
        .saturating_sub(thread_area.height.max(1) as usize)
}

fn focused_pane_rows(state: &AppState) -> (Option<usize>, Vec<roster_view::PaneRosterRow>) {
    let backend_filter = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.backend_filter.clone());
    let pane_clone = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .cloned();
    let pane_idx = state.multipane.as_ref().map(|mp| mp.focused);
    let Some(pane) = pane_clone else {
        return (pane_idx, Vec::new());
    };
    let rows = roster_view::compute_rows(state, &pane, backend_filter.as_deref());
    (pane_idx, rows)
}

fn move_roster_cursor(state: &mut AppState, delta: i32) {
    let (_, rows) = focused_pane_rows(state);
    let stops = roster_view::selectable_count(&rows);
    if stops == 0 {
        return;
    }
    let cursor = focused_pane_mut(state)
        .map(|p| p.roster_cursor as i32)
        .unwrap_or(0);
    let next = (cursor + delta).clamp(0, stops as i32 - 1) as usize;
    let row = roster_view::row_at_cursor(&rows, next).cloned();
    if let Some(pane) = focused_pane_mut(state) {
        pane.roster_cursor = next;
        roster_view::sync_tree_selection(pane, row.as_ref());
        roster_view::sync_auto_expansion(pane, row.as_ref());
    }
}

fn jump_roster_cursor_to_top(state: &mut AppState) {
    let (_, rows) = focused_pane_rows(state);
    let row = roster_view::row_at_cursor(&rows, 0).cloned();
    if let Some(pane) = focused_pane_mut(state) {
        pane.roster_cursor = 0;
        pane.roster_scroll = 0;
        roster_view::sync_tree_selection(pane, row.as_ref());
        roster_view::sync_auto_expansion(pane, row.as_ref());
    }
}

fn jump_roster_cursor_to_bottom(state: &mut AppState) {
    let (_, rows) = focused_pane_rows(state);
    let stops = roster_view::selectable_count(&rows);
    if stops == 0 {
        return;
    }
    let cursor = stops - 1;
    let row = roster_view::row_at_cursor(&rows, cursor).cloned();
    if let Some(pane) = focused_pane_mut(state) {
        pane.roster_cursor = cursor;
        roster_view::sync_tree_selection(pane, row.as_ref());
        roster_view::sync_auto_expansion(pane, row.as_ref());
    }
}

fn collapse_at_cursor(state: &mut AppState) {
    let Some(row) = row_under_focused_cursor(state) else {
        return;
    };
    match row {
        roster_view::PaneRosterRow::Backend { .. } => {
            // Drilling the cursor up off a Backend row clears
            // auto_expanded_backend through sync_auto_expansion, which
            // is the only source of "is this backend visible?".
            move_roster_cursor(state, -1);
        }
        roster_view::PaneRosterRow::Agent { agent_id, .. } => {
            if let Some(pane) = focused_pane_mut(state) {
                pane.roster_collapsed_agent_ids.insert(agent_id);
            }
        }
        _ => {}
    }
    clamp_focused_roster_cursor(state);
}

fn expand_at_cursor(state: &mut AppState) {
    let Some(row) = row_under_focused_cursor(state) else {
        return;
    };
    match row {
        roster_view::PaneRosterRow::Backend { .. } => {
            // Drilling cursor down to the first child auto-expands the
            // parent backend through sync_auto_expansion.
            move_roster_cursor(state, 1);
        }
        roster_view::PaneRosterRow::Agent { agent_id, .. } => {
            if let Some(pane) = focused_pane_mut(state) {
                pane.roster_collapsed_agent_ids.remove(&agent_id);
            }
        }
        _ => {}
    }
}

fn clamp_focused_roster_cursor(state: &mut AppState) {
    let (_, rows) = focused_pane_rows(state);
    let stops = roster_view::selectable_count(&rows);
    if let Some(pane) = focused_pane_mut(state) {
        if stops == 0 {
            pane.roster_cursor = 0;
        } else if pane.roster_cursor >= stops {
            pane.roster_cursor = stops - 1;
        }
    }
}

fn row_under_focused_cursor(state: &AppState) -> Option<roster_view::PaneRosterRow> {
    let (_, rows) = focused_pane_rows(state);
    let cursor = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .map(|p| p.roster_cursor)
        .unwrap_or(0);
    roster_view::row_at_cursor(&rows, cursor).cloned()
}

fn toggle_size_at_cursor(state: &mut AppState) {
    let Some(roster_view::PaneRosterRow::SizeLeaf {
        agent_id, leaf_idx, ..
    }) = row_under_focused_cursor(state)
    else {
        return;
    };
    let pane_idx = state.multipane.as_ref().map(|mp| mp.focused).unwrap_or(0);
    roster_view::toggle_size_leaf(state, pane_idx, &agent_id, leaf_idx);
}

fn commit_roster_selection(
    state: &mut AppState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
) {
    let _ = (codex, claude, swarm);
    let (pane_idx, rows) = focused_pane_rows(state);
    let cursor = focused_pane_mut(state)
        .map(|p| p.roster_cursor)
        .unwrap_or(0);
    let Some(row) = roster_view::row_at_cursor(&rows, cursor).cloned() else {
        push_pane_system_message(state, "no agents available to select".into());
        return;
    };
    let pane_idx = pane_idx.unwrap_or(0);
    dispatch_commit(state, pane_idx, row);
}

fn dispatch_commit(state: &mut AppState, pane_idx: usize, row: roster_view::PaneRosterRow) {
    match row {
        roster_view::PaneRosterRow::Backend { .. } => {
            // Enter on a Backend row drills the cursor down into the
            // group's first child, mirroring `l` / Right.
            if pane_idx == focused_pane_idx(state) {
                move_roster_cursor(state, 1);
            }
        }
        roster_view::PaneRosterRow::SizeBranch { agent_id } => {
            let Some(pane) = pane_at_mut(state, pane_idx) else {
                return;
            };
            roster_view::toggle_agent_tree_collapse(pane, &agent_id);
        }
        roster_view::PaneRosterRow::SizeLeaf {
            agent_id, leaf_idx, ..
        } => {
            roster_view::toggle_size_leaf(state, pane_idx, &agent_id, leaf_idx);
        }
        roster_view::PaneRosterRow::Agent { agent_id, .. } => {
            commit_agent_to_pane(state, pane_idx, &agent_id);
        }
        roster_view::PaneRosterRow::Template
        | roster_view::PaneRosterRow::Mission
        | roster_view::PaneRosterRow::Empty(_)
        | roster_view::PaneRosterRow::Spacer => {}
    }
}

fn revert_focused_pane_to_roster(state: &mut AppState) {
    if let Some(pane) = focused_pane_mut(state) {
        let pane_idx = pane.pane_id;
        pane.selected_agent_id = None;
        pane.agent_id.clear();
        pane.chat_input.clear();
        pane.chat_input_cursor = 0;
        pane.chat_input_selection_anchor = None;
        pane.chat_input_scroll = 0;
        pane.mission_id = None;
        pane.mission_ids.clear();
        // Re-derive the synthetic chat id so subsequent default-chat
        // dispatches (after re-committing an agent) still tag with a
        // stable per-pane id.
        pane.chat_mission_id = super::agent_id::pane_chat_mission_id(pane_idx);
        // Clear staleness from the cursor-driven latches and the dir
        // search overlay so a re-entered roster does not flash stale
        // state.
        pane.auto_expanded_backend = None;
        pane.auto_expanded_agent = None;
        pane.dir_search = None;
    }
}

fn handle_mouse(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
    clipboard: &mut Option<arboard::Clipboard>,
    area: Rect,
    mouse: MouseEvent,
) {
    // The renderer reserves the top status strip and bottom hint row
    // before painting panes (see `render_grid`). Click hit-tests must
    // strip the same chrome — without this, `pane_at_point` returns the
    // pane one row above the cursor, so clicking `Backend(Claude)` lands
    // on `Backend(Codex)` etc.
    let grid_area = if area.height >= 4 {
        Rect::new(area.x, area.y + 1, area.width, area.height - 2)
    } else {
        area
    };
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            handle_mouse_left_down(state, swarm, theme, clipboard, grid_area, mouse);
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            handle_mouse_left_drag(state, swarm, theme, clipboard, grid_area, mouse);
        }
        MouseEventKind::Up(MouseButton::Left) => {
            handle_mouse_left_up(state, swarm, grid_area, mouse.column, mouse.row);
        }
        MouseEventKind::ScrollUp => {
            handle_mouse_scroll(state, swarm, grid_area, mouse.column, mouse.row, -3);
        }
        MouseEventKind::ScrollDown => {
            handle_mouse_scroll(state, swarm, grid_area, mouse.column, mouse.row, 3);
        }
        _ => {}
    }
}

// Per-thread anchor for an in-progress popup body selection. Mirrors
// `InputState::mouse_select_anchor` from the single-pane handler. Stored
// here rather than on `AppState` because the anchor's lifetime is bounded
// by a single mouse-down → drag → up gesture, so leaking it across
// multipane / single-pane boundaries would just be noise.
thread_local! {
    static POPUP_BODY_ANCHOR: std::cell::Cell<Option<(usize, usize)>>
        = const { std::cell::Cell::new(None) };
    /// Per-gesture sentinel: which pane currently owns an in-progress
    /// chat-input-box drag. Mirrors single-pane's
    /// `MouseSelectTarget::ChatInput` flag — when Some, drag events
    /// extend the input-box selection on that pane rather than the
    /// chat-thread selection.
    static INPUT_BOX_DRAG_PANE: std::cell::Cell<Option<usize>>
        = const { std::cell::Cell::new(None) };
}

fn record_popup_anchor(line: usize, col: usize) {
    POPUP_BODY_ANCHOR.with(|cell| cell.set(Some((line, col))));
}

fn read_popup_anchor() -> Option<(usize, usize)> {
    POPUP_BODY_ANCHOR.with(|cell| cell.get())
}

fn clear_popup_anchor() {
    POPUP_BODY_ANCHOR.with(|cell| cell.set(None));
}

/// Lightweight equivalent of `app::ui_selection::update_ui_selection_text`
/// without the `InputState`-backed dedup. Multipane's popup selection
/// gesture is bounded (down → drag → up), and writing the same text
/// twice in a row to the clipboard is harmless, so we skip the
/// signature cache and copy on every selection change.
fn copy_popup_selection_to_clipboard(
    state: &mut AppState,
    lines: &[String],
    clipboard: &mut Option<arboard::Clipboard>,
) {
    let Some(selection) = state.ui_selection else {
        return;
    };
    if selection.pane != UiSelectionPane::ArtifactsPopup {
        return;
    }
    let text = crate::app::selection_text(lines, selection);
    if text.is_empty() {
        return;
    }
    state.yank = Some(text.clone());
    state.yank_kind = if text.contains('\n') {
        nit_core::YankKind::Line
    } else {
        nit_core::YankKind::Char
    };
    if let Some(cb) = clipboard.as_mut() {
        let _ = cb.set_text(text);
    }
}

fn handle_mouse_left_down(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
    clipboard: &mut Option<arboard::Clipboard>,
    area: Rect,
    mouse: MouseEvent,
) {
    let x = mouse.column;
    let y = mouse.row;
    if state.agents.artifacts_popup_open {
        // Mirror the single-pane handler at `app/mouse.rs:779-836`:
        //   1. Click inside popup body → seed a text-selection anchor
        //      and start a single-point UiSelection. Drag extends it.
        //   2. Click on the popup chat input → cursor positioning +
        //      input-buffer selection anchor.
        //   3. Click outside the popup → close popup.
        // Multipane stores the body anchor in a thread_local
        // (`POPUP_BODY_ANCHOR`) instead of `InputState`, but the
        // selection state lives on `AppState.ui_selection` like the
        // single-pane case, so the renderer highlights identically.
        let popup_area = popup_rect_for(area, artifacts_popup::preferred_size(area));

        // Chat input within the popup — cursor positioning + selection
        // anchor on the input buffer (matches single-pane behaviour).
        if let Some(cursor_char_idx) =
            artifacts_popup::map_chat_input_point_to_cursor(state, swarm, popup_area, x, y, false)
        {
            let total_chars = state.agents.artifacts_popup_chat_input.chars().count();
            let new_cursor = cursor_char_idx.min(total_chars);
            if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                if state.agents.artifacts_popup_chat_selection_anchor.is_none() {
                    state.agents.artifacts_popup_chat_selection_anchor =
                        Some(state.agents.artifacts_popup_chat_cursor.min(total_chars));
                }
            } else {
                state.agents.artifacts_popup_chat_selection_anchor = Some(new_cursor);
            }
            state.agents.artifacts_popup_chat_cursor = new_cursor;
            // Body anchor reset — clicking the input clears any pending
            // body drag.
            clear_popup_anchor();
            // The popup chat input has its own selection-text path
            // separate from `ui_selection`; leave that to keypath
            // handlers for now (out of scope this round).
            return;
        }

        // Click on a body line → start a body selection.
        if let Some((line_idx, col, lines)) = crate::app::map_artifacts_popup_mouse_with_swarm(
            swarm, mouse, area, state, theme, false,
        ) {
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::ArtifactsPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            record_popup_anchor(line_idx, col);
            copy_popup_selection_to_clipboard(state, &lines, clipboard);
            return;
        }

        // Inside popup but neither chat-input nor body — likely a
        // border / padding click. No-op (don't close).
        if point_in_rect(x, y, popup_area) {
            return;
        }
        // Outside the popup → close.
        state.agents.artifacts_popup_open = false;
        clear_popup_anchor();
        if matches!(state.ui_selection, Some(s) if s.pane == UiSelectionPane::ArtifactsPopup) {
            state.ui_selection = None;
        }
        return;
    }
    // Input-box click: position cursor + seed input selection anchor.
    // Mirrors single-pane behaviour at `app/mouse.rs:1127-1148`. Tested
    // BEFORE the chat-thread hit-test so an input-box click never spills
    // into the thread-selection branch.
    if let Some((pane_idx, cursor_char_idx)) = resolve_pane_input_box_hit(state, area, x, y) {
        if let Some(mp) = state.multipane.as_mut() {
            mp.focused = pane_idx;
        }
        with_pane_aliased(state, pane_idx, |state| {
            let total_chars = state.agents.chat_input.chars().count();
            let new_cursor = cursor_char_idx.min(total_chars);
            if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                if state.agents.chat_input_selection_anchor.is_none() {
                    state.agents.chat_input_selection_anchor =
                        Some(state.agents.chat_input_cursor.min(total_chars));
                }
            } else {
                state.agents.chat_input_selection_anchor = Some(new_cursor);
            }
            state.agents.chat_input_cursor = new_cursor;
            // Mirror single-pane (`app/mouse.rs:1153`): every selection
            // mutation auto-copies, so a shift-click that grew the
            // selection is immediately on the clipboard without the
            // operator having to also press Cmd+C.
            crate::app::copy_chat_input_selection(state, clipboard);
        });
        INPUT_BOX_DRAG_PANE.with(|cell| cell.set(Some(pane_idx)));
        return;
    }
    // Drag-to-select takes precedence over artifact-popup open: seed
    // the selection anchor on Down, defer popup-open to Up so a single
    // click without drag still opens the popup. Selection lives on the
    // pane that owns the click — never the focused pane — so dragging
    // inside an unfocused pane creates a per-pane selection.
    //
    // The *swarm-aware* resolver is critical here: when the pane's
    // `chat_thread_scroll == CONSOLE_SCROLL_BOTTOM` (the "follow
    // bottom" sentinel — true for any pane that hasn't been scrolled
    // by hand), the no-swarm variant treats it as `0` and the
    // selection lands on the wrong row. Passing the swarm runtime
    // resolves the sentinel to the actual `max_scroll` so the row
    // index lines up with what the renderer painted.
    if let Some((pane_idx, line_idx, col_idx)) =
        resolve_chat_thread_hit_with_swarm(state, Some(swarm), area, x, y)
    {
        let Some(pane) = pane_at_mut(state, pane_idx) else {
            return;
        };
        selection::clear(pane);
        selection::extend_to(pane, line_idx, col_idx);
        return;
    }
    let Some(target) = resolve_left_click_target(state, area, x, y) else {
        return;
    };
    apply_roster_click(state, target);
}

/// Resolve a screen `(x, y)` to `(pane_idx, char_index_into_chat_input)`
/// when the click lands inside any pane's chat input box. Mirrors
/// `resolve_chat_thread_hit_with_swarm` but for the input rect produced
/// by `compute_pane_layout`.
fn resolve_pane_input_box_hit(
    state: &AppState,
    area: Rect,
    x: u16,
    y: u16,
) -> Option<(usize, usize)> {
    let mp = state.multipane.as_ref()?;
    let pane_idx = grid::pane_at_point(area, mp.grid_cols, mp.grid_rows, x, y)?;
    let pane = mp.panes.get(pane_idx)?;
    if pane.selected_agent_id.is_none() && pane.agent_id.is_empty() {
        return None;
    }
    let pane_rect = grid::pane_rect(area, mp.grid_cols, mp.grid_rows, pane_idx);
    let inner = pane_inner_after_chrome(pane_rect);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let cursor_char_idx =
        agent_console_view::map_pane_chat_input_point_to_cursor(inner, pane, x, y, false)?;
    Some((pane_idx, cursor_char_idx))
}

fn handle_mouse_left_drag(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
    clipboard: &mut Option<arboard::Clipboard>,
    area: Rect,
    mouse: MouseEvent,
) {
    let x = mouse.column;
    let y = mouse.row;

    // Popup body drag: extend the anchor → current point UiSelection
    // and re-copy to clipboard. Mirrors the single-pane drag handler
    // (`app/mouse.rs::handle_mouse_drag_with_swarm` UiSelectionPane
    // branch) but reads the anchor from the multipane thread_local
    // since `InputState` isn't plumbed here.
    if state.agents.artifacts_popup_open {
        if let Some((anchor_line, anchor_col)) = read_popup_anchor() {
            if let Some((line_idx, col, lines)) = crate::app::map_artifacts_popup_mouse_with_swarm(
                swarm, mouse, area, state, theme, true,
            ) {
                state.ui_selection = Some(UiSelection {
                    pane: UiSelectionPane::ArtifactsPopup,
                    start_line: anchor_line,
                    start_col: anchor_col,
                    end_line: line_idx,
                    end_col: col,
                });
                copy_popup_selection_to_clipboard(state, &lines, clipboard);
            }
        }
        return;
    }

    // Input-box drag: extend the chat-input selection. The thread_local
    // sentinel (not the chat-thread hit-test) keeps the drag targeted at
    // the input box even when the cursor moves outside its rect; clamping
    // is enabled so the selection expands to the row/col edge.
    if let Some(pane_idx) = INPUT_BOX_DRAG_PANE.with(|cell| cell.get()) {
        let pane = match state
            .multipane
            .as_ref()
            .and_then(|mp| mp.panes.get(pane_idx))
        {
            Some(pane) => pane.clone(),
            None => {
                INPUT_BOX_DRAG_PANE.with(|cell| cell.set(None));
                return;
            }
        };
        let mp_grid = state
            .multipane
            .as_ref()
            .map(|mp| (mp.grid_cols, mp.grid_rows));
        let Some((grid_cols, grid_rows)) = mp_grid else {
            return;
        };
        let pane_rect = grid::pane_rect(area, grid_cols, grid_rows, pane_idx);
        let inner = pane_inner_after_chrome(pane_rect);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        if let Some(cursor_char_idx) =
            agent_console_view::map_pane_chat_input_point_to_cursor(inner, &pane, x, y, true)
        {
            with_pane_aliased(state, pane_idx, |state| {
                let total_chars = state.agents.chat_input.chars().count();
                let new_cursor = cursor_char_idx.min(total_chars);
                if state.agents.chat_input_selection_anchor.is_none() {
                    state.agents.chat_input_selection_anchor =
                        Some(state.agents.chat_input_cursor.min(total_chars));
                }
                state.agents.chat_input_cursor = new_cursor;
                // Match single-pane drag behaviour (`app/mouse.rs:1641`)
                // — auto-copy on every drag tick so releasing the mouse
                // leaves the selection already on the clipboard.
                crate::app::copy_chat_input_selection(state, clipboard);
            });
        }
        return;
    }

    // Same sentinel concern as the Down handler: a drag whose start
    // point landed on a sentinel-scrolled pane needs the swarm-aware
    // resolver, otherwise `chat_thread_scroll` is treated as 0 and the
    // selection extends to a row that has nothing to do with what's
    // visually under the cursor.
    let Some((pane_idx, line_idx, col_idx)) =
        resolve_chat_thread_hit_with_swarm(state, Some(swarm), area, x, y)
    else {
        return;
    };
    let owns_anchor = pane_at(state, pane_idx)
        .and_then(|p| p.selection.as_ref().map(|_| ()))
        .is_some();
    if !owns_anchor {
        return;
    }
    if let Some(pane) = pane_at_mut(state, pane_idx) {
        selection::extend_to(pane, line_idx, col_idx);
    }
}

fn handle_mouse_left_up(state: &mut AppState, swarm: &SwarmRuntime, area: Rect, x: u16, y: u16) {
    // Popup body anchor lives only for the duration of the gesture.
    // Drop it on Up regardless of pane hit so a stray release outside
    // the popup doesn't leak the anchor across gestures.
    if state.agents.artifacts_popup_open {
        clear_popup_anchor();
        return;
    }
    // Input-box drag terminator: drop the per-gesture sentinel so the
    // next mouse-down starts a fresh selection. The pane's
    // chat_input_selection_anchor stays set if the drag covered any
    // characters — Cmd+C / Ctrl+C on the canonical handler picks it up.
    if INPUT_BOX_DRAG_PANE
        .with(|cell| cell.replace(None))
        .is_some()
    {
        return;
    }
    let Some((pane_idx, _, _)) = resolve_chat_thread_hit_with_swarm(state, Some(swarm), area, x, y)
    else {
        return;
    };
    let collapsed = pane_at(state, pane_idx)
        .and_then(|p| p.selection.as_ref())
        .map(|s| (s.anchor_line, s.anchor_col) == (s.end_line, s.end_col))
        .unwrap_or(true);
    if !collapsed {
        // Real drag: keep the selection — Ctrl/Cmd+C copies it later.
        return;
    }
    if let Some(pane) = pane_at_mut(state, pane_idx) {
        selection::clear(pane);
    }
    if try_open_chat_pane_artifact(state, swarm, area, x, y) {
        return;
    }
    if let Some(target) = resolve_left_click_target(state, area, x, y) {
        apply_roster_click(state, target);
    }
}

/// Resolve a screen `(x, y)` to `(pane_idx, logical_line, char_col)`
/// inside that pane's chat thread. `logical_line` already includes
/// `chat_thread_scroll`, so it's directly usable as a row index into
/// `build_pane_thread_rows`. Returns `None` if the click lands outside
/// the pane's chat thread area, or the pane is in roster mode. The
/// `swarm` argument is required when `chat_thread_scroll` may hold the
/// `CONSOLE_SCROLL_BOTTOM` sentinel (true for any pane that hasn't been
/// scrolled by hand) — without it, sentinel resolution falls back to 0
/// and the resolved line is wrong by `max_scroll` rows. Pass `None`
/// only when the caller is certain the sentinel never applies (tests
/// that pre-set scroll to a numeric value).
///
/// Sentinel-resolution: when
/// `pane.chat_thread_scroll == CONSOLE_SCROLL_BOTTOM`, we must
/// translate it to `max_scroll` BEFORE adding `local_y` — otherwise
/// the logical row ends up at `usize::MAX` and the artifact-popup
/// resolver fails to find anything at that line.
fn resolve_chat_thread_hit_with_swarm(
    state: &AppState,
    swarm: Option<&SwarmRuntime>,
    area: Rect,
    x: u16,
    y: u16,
) -> Option<(usize, usize, usize)> {
    let mp = state.multipane.as_ref()?;
    let pane_idx = grid::pane_at_point(area, mp.grid_cols, mp.grid_rows, x, y)?;
    let pane = mp.panes.get(pane_idx)?;
    if pane.selected_agent_id.is_none() && pane.agent_id.is_empty() {
        return None;
    }
    let thread_area = pane_thread_area_for_pane(state, area, pane_idx, pane)?;
    if !point_in_rect(x, y, thread_area) {
        return None;
    }
    let local_y = (y - thread_area.y) as usize;
    let local_x = (x - thread_area.x) as usize;
    // Resolve the sentinel "follow bottom" to a concrete row offset.
    // Falls back to a clamp against the renderer's max_scroll when a
    // swarm runtime is available; without one, treat the sentinel as
    // 0 so the click maps somewhere reasonable rather than overflowing.
    let scroll = if pane.chat_thread_scroll == nit_core::CONSOLE_SCROLL_BOTTOM {
        match swarm {
            Some(s) => focused_pane_chat_thread_max_scroll(state, s, pane, area, pane_idx),
            None => 0,
        }
    } else {
        pane.chat_thread_scroll
    };
    Some((pane_idx, scroll.saturating_add(local_y), local_x))
}

fn point_in_rect(x: u16, y: u16, rect: Rect) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

/// If `(x, y)` lands on a chat-pane thread row that the artifact-popup
/// resolver recognises, open the popup and return `true`. Otherwise
/// returns `false` so the caller can fall through to roster click
/// resolution. Mirrors the layout arithmetic used by
/// [`agent_console_view::render_pane`] so the row index lines up with
/// what the renderer painted.
fn try_open_chat_pane_artifact(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    area: Rect,
    x: u16,
    y: u16,
) -> bool {
    // Use the swarm-aware resolver: when `chat_thread_scroll` holds
    // the "follow bottom" sentinel, this translates it to the actual
    // max_scroll so the resulting `line_idx` lines up with what the
    // renderer painted (and what the artifact-popup resolver expects).
    let Some((pane_idx, line_idx, _col)) =
        resolve_chat_thread_hit_with_swarm(state, Some(swarm), area, x, y)
    else {
        return false;
    };
    let Some(pane) = pane_at(state, pane_idx).cloned() else {
        return false;
    };
    let Some(thread_area) = pane_thread_area_for_pane(state, area, pane_idx, &pane) else {
        return false;
    };
    invoke_pane_artifact_popup(
        state,
        swarm,
        pane_idx,
        &pane,
        thread_area.width as usize,
        line_idx,
    )
}

/// Chat thread paint rect — a thin layer above [`pane_body_rect`]
/// that adds the prompt-input split.
fn pane_thread_area_for_pane(
    state: &AppState,
    area: Rect,
    pane_idx: usize,
    pane: &nit_core::PaneSession,
) -> Option<Rect> {
    let body = pane_body_rect(state, area, pane_idx, pane)?;
    agent_console_view::pane_thread_text_area(body, pane)
}

/// Alias `selected_*` to `pane` for the popup-open call so the resolver
/// (`artifact_message_index_for_line_with_swarm` via `selected_context_*`)
/// walks this pane's mission/agent. On a successful open `popup_keys`
/// deliberately writes `selected_mission` to bind the popup to the
/// clicked artifact — leave those values in place. Restore on miss so
/// other panes don't see contaminated globals.
fn invoke_pane_artifact_popup(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    pane_idx: usize,
    pane: &nit_core::PaneSession,
    text_width: usize,
    line_idx: usize,
) -> bool {
    let saved_agent = state.agents.selected_agent.clone();
    let saved_mission = state.agents.selected_mission.clone();
    // Stamp the mission_selected sentinel for parity with with_pane_aliased
    // — without this, selected_context_mission()'s missions[mission_selected]
    // fallback can return another pane's mission and the artifact popup
    // resolver walks the wrong thread.
    let saved_mission_selected = state.agents.mission_selected;
    let pane_agent_id = if pane.agent_id.is_empty() {
        pane.selected_agent_id.clone()
    } else {
        Some(pane.agent_id.clone())
    };
    state.agents.selected_agent = pane_agent_id;
    state.agents.selected_mission = pane.mission_id.clone();
    state.agents.mission_selected = usize::MAX;
    // Pane-aware variant: the resolver must walk the same pane-scoped
    // message list the renderer used (`message_matches_pane`),
    // otherwise an inline breather (e.g. active shadow run) shifts the
    // row cursor and clicking `(see ARTIFACTS)` misses entirely.
    let opened = crate::app::popup_keys::maybe_open_artifact_popup_from_console_line_for_pane(
        state,
        Some(swarm),
        Some(pane_idx),
        text_width,
        line_idx,
    );
    if !opened {
        state.agents.selected_agent = saved_agent;
        state.agents.selected_mission = saved_mission;
        state.agents.mission_selected = saved_mission_selected;
    }
    opened
}

struct RosterClickTarget {
    pane_idx: usize,
    rows: Vec<roster_view::PaneRosterRow>,
    row_idx: usize,
    row: roster_view::PaneRosterRow,
    local_x: usize,
}

fn resolve_left_click_target(
    state: &mut AppState,
    area: Rect,
    x: u16,
    y: u16,
) -> Option<RosterClickTarget> {
    let mp = state.multipane.as_mut()?;
    let pane_idx = focus::focus_at_point(mp, area, x, y)?;
    let backend_filter = mp.backend_filter.clone();
    let pane = mp.panes.get(pane_idx).cloned()?;
    if !(pane.selected_agent_id.is_none() && pane.agent_id.is_empty()) {
        return None; // chat panes ignore left-clicks beyond focus
    }
    let body = pane_body_rect(state, area, pane_idx, &pane)?;
    if !point_in_rect(x, y, body) {
        return None;
    }
    let local_x = (x - body.x) as usize;
    let local_y = (y - body.y) as usize;
    let rows = roster_view::compute_rows(state, &pane, backend_filter.as_deref());
    let row_idx = roster_view::row_index_at_y(&rows, pane.roster_scroll, local_y)?;
    let row = rows.get(row_idx).cloned()?;
    Some(RosterClickTarget {
        pane_idx,
        rows,
        row_idx,
        row,
        local_x,
    })
}

/// Single source of truth for "where this pane's content paints" —
/// chrome stripped + dir-search overlay stripped. Roster panes paint
/// rows directly into this rect; chat panes split it further into
/// thread + input via [`pane_thread_area_for_pane`].
fn pane_body_rect(
    state: &AppState,
    area: Rect,
    pane_idx: usize,
    pane: &nit_core::PaneSession,
) -> Option<Rect> {
    let mp = state.multipane.as_ref()?;
    let pane_rect = grid::pane_rect(area, mp.grid_cols, mp.grid_rows, pane_idx);
    let inner = pane_inner_after_chrome(pane_rect);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let body = dir_search_body_rect(inner, pane);
    (body.width > 0 && body.height > 0).then_some(body)
}

fn apply_roster_click(state: &mut AppState, target: RosterClickTarget) {
    let RosterClickTarget {
        pane_idx,
        rows,
        row_idx,
        row,
        local_x,
    } = target;
    match row {
        roster_view::PaneRosterRow::Template => {
            if let Some(value) = roster_view::template_word_at_x(local_x) {
                if let Some(pane) = pane_at_mut(state, pane_idx) {
                    pane.swarm_template = value.into();
                }
            }
        }
        roster_view::PaneRosterRow::Mission => {
            if let Some(value) = roster_view::mission_word_at_x(local_x) {
                if let Some(pane) = pane_at_mut(state, pane_idx) {
                    pane.swarm_mission = value.into();
                }
            }
        }
        roster_view::PaneRosterRow::Backend { .. } => {
            seek_pane_cursor_to(state, pane_idx, &rows, row_idx);
        }
        roster_view::PaneRosterRow::Agent { agent_id, .. } => {
            seek_pane_cursor_to(state, pane_idx, &rows, row_idx);
            commit_agent_to_pane(state, pane_idx, &agent_id);
        }
        roster_view::PaneRosterRow::SizeBranch { agent_id } => {
            seek_pane_cursor_to(state, pane_idx, &rows, row_idx);
            if let Some(pane) = pane_at_mut(state, pane_idx) {
                roster_view::toggle_agent_tree_collapse(pane, &agent_id);
            }
        }
        roster_view::PaneRosterRow::SizeLeaf {
            agent_id, leaf_idx, ..
        } => {
            seek_pane_cursor_to(state, pane_idx, &rows, row_idx);
            roster_view::toggle_size_leaf(state, pane_idx, &agent_id, leaf_idx);
        }
        roster_view::PaneRosterRow::Empty(_) | roster_view::PaneRosterRow::Spacer => {}
    }
}

fn seek_pane_cursor_to(
    state: &mut AppState,
    pane_idx: usize,
    rows: &[roster_view::PaneRosterRow],
    row_idx: usize,
) {
    let Some(cursor) = roster_view::cursor_for_row_index(rows, row_idx) else {
        return;
    };
    let row = rows.get(row_idx).cloned();
    if let Some(pane) = pane_at_mut(state, pane_idx) {
        pane.roster_cursor = cursor;
        roster_view::sync_tree_selection(pane, row.as_ref());
        roster_view::sync_auto_expansion(pane, row.as_ref());
    }
}

fn commit_agent_to_pane(state: &mut AppState, pane_idx: usize, agent_id: &str) {
    let message = match materialise_pane_lane(state, pane_idx, agent_id) {
        Some(id) => format!("selected agent → {id}"),
        None => format!("could not materialise pane lane for {agent_id}"),
    };
    push_pane_system_message(state, message);
}

fn handle_mouse_scroll(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    area: Rect,
    x: u16,
    y: u16,
    delta: i32,
) {
    // Modal: wheel events while the artifacts popup is open scroll the
    // popup, not the chat thread underneath. Match `app/mouse.rs`.
    if state.agents.artifacts_popup_open {
        let popup_area = popup_rect_for(area, artifacts_popup::preferred_size(area));
        if point_in_rect(x, y, popup_area) {
            let max_scroll = state.agents.artifacts_popup_last_max_scroll;
            // The renderer re-clamps each frame, so a stale scroll value
            // self-corrects on next draw — safe to advance optimistically.
            let current = state.agents.artifacts_popup_scroll as i32;
            let next = (current + delta).max(0) as usize;
            state.agents.artifacts_popup_scroll = next.min(max_scroll.max(0));
        }
        return;
    }
    let Some(mp) = state.multipane.as_ref() else {
        return;
    };
    let Some(pane_idx) = grid::pane_at_point(area, mp.grid_cols, mp.grid_rows, x, y) else {
        return;
    };
    let Some(pane) = mp.panes.get(pane_idx).cloned() else {
        return;
    };
    let in_roster = pane.selected_agent_id.is_none() && pane.agent_id.is_empty();
    let max_scroll = if in_roster {
        roster_max_scroll(state, &pane, area, pane_idx)
    } else {
        focused_pane_chat_thread_max_scroll(state, swarm, &pane, area, pane_idx)
    };
    let Some(p) = pane_at_mut(state, pane_idx) else {
        return;
    };
    if in_roster {
        let current = p.roster_scroll as i32;
        let next = (current + delta).max(0) as usize;
        p.roster_scroll = next.min(max_scroll);
    } else {
        // Wheel uses the same sentinel-resolution path as the keyboard
        // scroll — see `resolve_chat_scroll_sentinel` and `scroll_chat_thread`
        // for the matching delta-guard rationale.
        let resolved = resolve_chat_scroll_sentinel(p.chat_thread_scroll, max_scroll);
        let next = (resolved as i32 + delta).max(0) as usize;
        p.chat_thread_scroll = if delta > 0 && next >= max_scroll {
            nit_core::CONSOLE_SCROLL_BOTTOM
        } else {
            next.min(max_scroll)
        };
    }
}

fn roster_max_scroll(
    state: &AppState,
    pane: &nit_core::PaneSession,
    area: Rect,
    pane_idx: usize,
) -> usize {
    let height = pane_body_rect(state, area, pane_idx, pane)
        .map(|body| body.height as usize)
        .unwrap_or(0);
    let backend_filter = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.backend_filter.clone());
    let rows = roster_view::compute_rows(state, pane, backend_filter.as_deref());
    rows.len().saturating_sub(height)
}

fn pane_inner_after_chrome(rect: Rect) -> Rect {
    if rect.width < 2 || rect.height < 2 {
        return Rect::new(rect.x, rect.y, 0, 0);
    }
    let inner = Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2);
    inner_rect_after_hint(inner)
}

/// Copy the focused pane's active chat-thread selection to the
/// clipboard. Returns `true` when a non-empty selection was copied (and
/// cleared); `false` otherwise so the caller can fall through to the
/// abort path. Width is computed from the focused pane's render area
/// so wrap boundaries match what the operator saw at drag time.
fn try_copy_focused_pane_selection(
    state: &mut AppState,
    clipboard: &mut Option<arboard::Clipboard>,
    area: Rect,
) -> bool {
    let Some(mp) = state.multipane.as_ref() else {
        return false;
    };
    let pane_idx = mp.focused;
    let Some(pane) = mp.panes.get(pane_idx).cloned() else {
        return false;
    };
    if pane.selection.is_none() {
        return false;
    }
    let in_chat_mode = pane.selected_agent_id.is_some() || !pane.agent_id.is_empty();
    if !in_chat_mode {
        return false;
    }
    let pane_rect = grid::pane_rect(area, mp.grid_cols, mp.grid_rows, pane_idx);
    let inner = pane_inner_after_chrome(pane_rect);
    let Some(thread_area) = agent_console_view::pane_thread_text_area(inner, &pane) else {
        return false;
    };
    let width = thread_area.width.max(1) as usize;
    let rows = agent_console_view::build_pane_thread_rows_for_pane(
        state,
        None,
        Some(pane.pane_id),
        Some(pane.agent_id.as_str()),
        pane.mission_id.as_deref().or_else(|| {
            (!pane.chat_mission_id.is_empty()).then_some(pane.chat_mission_id.as_str())
        }),
        width,
        !pane.has_run_mission,
    );
    let text = selection::resolve_text(&pane, &rows);
    let Some(text) = text else {
        if let Some(p) = pane_at_mut(state, pane_idx) {
            selection::clear(p);
        }
        return false;
    };
    if let Some(cb) = clipboard.as_mut() {
        let _ = cb.set_text(text);
    }
    if let Some(p) = pane_at_mut(state, pane_idx) {
        selection::clear(p);
    }
    true
}

fn focused_pane_mut(state: &mut AppState) -> Option<&mut nit_core::PaneSession> {
    let mp = state.multipane.as_mut()?;
    let idx = mp.focused;
    mp.panes.get_mut(idx)
}

fn focused_pane_idx(state: &AppState) -> usize {
    state.multipane.as_ref().map(|mp| mp.focused).unwrap_or(0)
}

fn focused_pane_in_roster_mode(state: &AppState) -> bool {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .map(|p| p.selected_agent_id.is_none() && p.agent_id.is_empty())
        .unwrap_or(false)
}

fn focused_chat_input_is_empty(state: &AppState) -> bool {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .map(|p| p.chat_input.trim().is_empty())
        .unwrap_or(true)
}

fn clear_focused_pane_input(state: &mut AppState) {
    if let Some(pane) = focused_pane_mut(state) {
        pane.chat_input.clear();
        pane.chat_input_cursor = 0;
        pane.chat_input_selection_anchor = None;
        pane.chat_input_scroll = 0;
    }
}

fn push_pane_system_message(state: &mut AppState, text: String) {
    let pane = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .cloned();
    let Some(pane) = pane else { return };
    let agent_id = if !pane.agent_id.is_empty() {
        Some(pane.agent_id.clone())
    } else {
        pane.selected_agent_id.clone()
    };
    let at = format!("t+{}", state.metrics.frame_count);
    state.agents.messages.push(AgentMessage {
        at,
        channel: AgentChannel::Agent,
        agent_id,
        mission_id: pane.mission_id.clone(),
        text,
        prompt_msg_idx: None,
        kind: Some("multipane-system".into()),
    });
}

/// Multipane abort. Routes through the canonical
/// `chat_input::handle_abort` so swarm missions roll over to
/// `completed_runs` with `report_status="ABORTED"`, queues drain via
/// `release_queued_slot`, and the system alert lands as a
/// `SYSTEM_ALERT_KIND` message — same semantics as the standard chat.
///
/// The pane's `mission_id` is aliased into `state.agents.selected_mission`
/// so `AbortScope::Current` resolves to the right mission. When a pane has
/// no real swarm mission (only the synthetic chat id), routes to a
/// surgical per-agent `CancelTurn` instead so the swarm-wide fallback in
/// `handle_abort` cannot reach into another pane's mission.
fn abort_focused_pane(
    state: &mut AppState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
) {
    let Some(focused_agent) = focused_pane_agent_id(state) else {
        push_pane_system_message(state, "no agent selected — nothing to abort".into());
        return;
    };
    // Inspect mission scope BEFORE entering with_focused_pane_aliased
    // because synthetic-id-only state must route to AbortScope::Agent
    // (per-agent CancelTurn), never AbortScope::Current.
    let pane = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .cloned();
    let Some(pane) = pane else {
        return;
    };
    let real_mission = pane
        .mission_id
        .as_deref()
        .filter(|m| !super::agent_id::is_pane_chat_mission_id(m))
        .map(|s| s.to_string())
        .or_else(|| {
            state
                .agents
                .agents_get(&focused_agent)
                .and_then(|lane| lane.current_mission.clone())
                .filter(|m| !super::agent_id::is_pane_chat_mission_id(m))
        });
    let swarm_active = real_mission
        .as_deref()
        .is_some_and(|mid| swarm.is_active_mission(mid));
    if swarm_active {
        // Alias places this pane's mission into selected_mission so
        // AbortScope::Current resolves to exactly this pane's swarm —
        // no cross-pane fallback.
        with_focused_pane_aliased(state, |state| {
            handle_abort(state, Some(codex), Some(claude), swarm, AbortScope::Current);
        });
        return;
    }
    // Single-agent shadow mode: a `@shadow` prompt (or auto-shadow on
    // a heavy single-agent prompt) spins up hidden propose-a /
    // propose-b / judge / review lanes. While they run, the *base*
    // lane is idle, so the lane-in-flight check below is false even
    // though the operator clearly sees activity in the breather.
    // Detect the shadow run first and tear it down before falling
    // through, otherwise `/abort` posts "no active mission for this
    // pane" while propose / judge keep burning tokens.
    if shadow.has_run_for(&focused_agent) {
        let shadow_lanes = shadow.abort_run(state, &focused_agent);
        // CancelTurn for each shadow lane — `cleanup_shadow_lanes`
        // (called inside `abort_run`) only purges in-process
        // bookkeeping; the runner subprocesses are still alive and
        // would otherwise keep streaming until they hit their idle
        // reaper.
        for lane_id in &shadow_lanes {
            let _ = codex.send(crate::codex_runner::CodexCommand::CancelTurn {
                agent_id: lane_id.clone(),
            });
            let _ = claude.send(crate::claude_runner::ClaudeCommand::CancelTurn {
                agent_id: lane_id.clone(),
            });
        }
        // Drain any queued main-agent turn the shadow pipeline was
        // about to dispatch once review finished.
        crate::swarm::drain_queued_turns_for_agent_pub(state, &focused_agent);
        push_pane_system_message(
            state,
            format!("aborted shadow run ({} lanes)", shadow_lanes.len()),
        );
        return;
    }
    // Stale mission id, or never had a real swarm overlay. Surgically
    // cancel the focused pane's lane via AbortScope::Agent if a turn is
    // live; otherwise post a "nothing to abort" system message.
    if !lane_has_in_flight_turn(state, &focused_agent) {
        push_pane_system_message(state, "no active mission for this pane".into());
        return;
    }
    with_focused_pane_aliased(state, |state| {
        handle_abort(
            state,
            Some(codex),
            Some(claude),
            swarm,
            AbortScope::Agent(focused_agent.clone()),
        );
    });
}

fn focused_pane_dir_search_active(state: &AppState) -> bool {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .map(|p| p.dir_search.is_some())
        .unwrap_or(false)
}

fn close_focused_dir_search(state: &mut AppState) {
    if let Some(pane) = focused_pane_mut(state) {
        pane.dir_search = None;
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn toggle_focused_pane_dir_search(state: &mut AppState, runner: &DirSearchRunner) {
    let gitignored = state.gitignored_dirs.clone();
    let Some(pane) = focused_pane_mut(state) else {
        return;
    };
    if pane.dir_search.take().is_some() {
        return;
    }
    let cwd = pane.cwd.clone();
    let parsed = dir_search::parse_query("", &cwd, home_dir().as_deref());
    let id = runner.query(parsed.base.clone(), parsed.needle, false, gitignored);
    pane.dir_search = Some(nit_core::DirSearchState {
        base: parsed.base,
        generation: id,
        ..Default::default()
    });
}

fn issue_dir_search_query(state: &mut AppState, runner: &DirSearchRunner) {
    let gitignored = state.gitignored_dirs.clone();
    let Some(pane) = focused_pane_mut(state) else {
        return;
    };
    let Some(ds) = pane.dir_search.as_mut() else {
        return;
    };
    let ParsedQuery { base, needle } =
        dir_search::parse_query(&ds.query, &pane.cwd, home_dir().as_deref());
    if base != ds.base {
        ds.expanded.clear();
    }
    let expanded = ds.expanded.clone();
    let id = runner.query_with_expanded(base.clone(), needle, ds.show_hidden, gitignored, expanded);
    ds.base = base;
    ds.generation = id;
    ds.results.clear();
    ds.selected = 0;
    ds.view_offset = 0;
}

fn handle_dir_search_key(
    state: &mut AppState,
    runner: &DirSearchRunner,
    key: KeyEvent,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
) -> bool {
    let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => handle_dir_search_esc(state, codex, claude, swarm, shadow),
        KeyCode::Enter => commit_dir_search(state),
        KeyCode::Up => with_focused_dir_search(state, move_selected_up),
        KeyCode::Down => with_focused_dir_search(state, move_selected_down),
        // IMPORTANT: Ctrl+chord arms must precede the catch-all
        // KeyCode::Char(ch) below — otherwise the char inserts into the
        // query and the chord silently fails.
        KeyCode::Char('j') if is_ctrl => with_focused_dir_search(state, move_selected_down),
        KeyCode::Char('k') if is_ctrl => with_focused_dir_search(state, move_selected_up),
        KeyCode::Right => {
            expand_dir_search_at_cursor(state);
            issue_dir_search_query(state, runner);
        }
        KeyCode::Char('l') if is_ctrl => {
            expand_dir_search_at_cursor(state);
            issue_dir_search_query(state, runner);
        }
        KeyCode::Left => {
            collapse_dir_search_at_cursor(state);
            issue_dir_search_query(state, runner);
        }
        KeyCode::Char('h') if is_ctrl => {
            collapse_dir_search_at_cursor(state);
            issue_dir_search_query(state, runner);
        }
        KeyCode::Home => with_focused_dir_search(state, |ds| ds.query_cursor = 0),
        KeyCode::End => with_focused_dir_search(state, |ds| {
            ds.query_cursor = ds.query.chars().count();
        }),
        KeyCode::Backspace => {
            with_focused_dir_search(state, mutate_backspace);
            issue_dir_search_query(state, runner);
        }
        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::ALT) => {
            with_focused_dir_search(state, |ds| ds.show_hidden = !ds.show_hidden);
            issue_dir_search_query(state, runner);
        }
        KeyCode::Char(ch) => {
            with_focused_dir_search(state, |ds| insert_query_char(ds, ch));
            issue_dir_search_query(state, runner);
        }
        _ => {}
    }
    false
}

fn move_selected_up(ds: &mut nit_core::DirSearchState) {
    if ds.results.is_empty() {
        return;
    }
    ds.selected = ds.selected.saturating_sub(1);
    clamp_viewport(ds, ds.last_visible as usize);
}

fn move_selected_down(ds: &mut nit_core::DirSearchState) {
    if ds.results.is_empty() {
        return;
    }
    let max = ds.results.len() - 1;
    ds.selected = (ds.selected + 1).min(max);
    clamp_viewport(ds, ds.last_visible as usize);
}

fn expand_dir_search_at_cursor(state: &mut AppState) {
    with_focused_dir_search(state, |ds| {
        if let Some(path) = ds.results.get(ds.selected).cloned() {
            if path.is_dir() {
                ds.expanded.insert(path);
            }
        }
    });
}

fn collapse_dir_search_at_cursor(state: &mut AppState) {
    with_focused_dir_search(state, |ds| {
        let Some(path) = ds.results.get(ds.selected).cloned() else {
            return;
        };
        if ds.expanded.remove(&path) {
            return;
        }
        let mut current: Option<&Path> = path.parent();
        while let Some(p) = current {
            if ds.expanded.remove(p) {
                return;
            }
            current = p.parent();
        }
    });
}

fn with_focused_dir_search<F: FnOnce(&mut nit_core::DirSearchState)>(state: &mut AppState, f: F) {
    let Some(pane) = focused_pane_mut(state) else {
        return;
    };
    let Some(ds) = pane.dir_search.as_mut() else {
        return;
    };
    f(ds);
}

fn mutate_backspace(ds: &mut nit_core::DirSearchState) {
    if ds.query_cursor == 0 {
        return;
    }
    let drop_at = ds.query_cursor - 1;
    ds.query = ds
        .query
        .chars()
        .enumerate()
        .filter_map(|(i, c)| (i != drop_at).then_some(c))
        .collect();
    ds.query_cursor = drop_at;
}

fn insert_query_char(ds: &mut nit_core::DirSearchState, ch: char) {
    let mut chars: Vec<char> = ds.query.chars().collect();
    let at = ds.query_cursor.min(chars.len());
    chars.insert(at, ch);
    ds.query = chars.into_iter().collect();
    ds.query_cursor = at + 1;
}

fn handle_dir_search_esc(
    state: &mut AppState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
) {
    // Esc closes the overlay and feeds the shared esc-press latch so a
    // second Esc within the abort window aborts the focused pane
    // (consistent with the chat-mode Esc handler).
    let double_tap = record_chat_esc_press();
    close_focused_dir_search(state);
    if !double_tap {
        return;
    }
    if focused_pane_in_roster_mode(state) {
        push_pane_system_message(state, "no agent selected — nothing to abort".into());
    } else {
        abort_focused_pane(state, codex, claude, swarm, shadow);
    }
    clear_chat_esc_state();
}

fn commit_dir_search(state: &mut AppState) {
    let chosen = take_dir_search_choice(state);
    let Some(path) = chosen else { return };
    if let Some(pane) = focused_pane_mut(state) {
        pane.cwd = path.clone();
    }
    invalidate_focused_pane_resume_sessions(state);
    push_pane_system_alert(state, format!("cwd → {}", path.display()));
}

/// Drop the focused pane's resume ids so a fresh session is created in
/// the new cwd. Otherwise CLI session metadata re-anchors the spawn cwd
/// to the original workspace silently.
fn invalidate_focused_pane_resume_sessions(state: &mut AppState) {
    let Some(agent_id) = focused_pane_agent_id(state) else {
        return;
    };
    let mission_id = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .and_then(|p| p.mission_id.clone());
    state.agents.codex_thread_ids.remove(&agent_id);
    state.agents.claude_session_ids.remove(&agent_id);
    if let Some(mid) = mission_id.as_deref() {
        if let Some(threads) = state.agents.codex_mission_thread_ids.get_mut(mid) {
            threads.remove(&agent_id);
        }
        if let Some(sessions) = state.agents.claude_mission_session_ids.get_mut(mid) {
            sessions.remove(&agent_id);
        }
    }
}

fn take_dir_search_choice(state: &mut AppState) -> Option<PathBuf> {
    let pane = focused_pane_mut(state)?;
    let candidate = pane
        .dir_search
        .as_ref()
        .and_then(|ds| ds.results.get(ds.selected).cloned());
    pane.dir_search = None;
    let path = candidate?;
    // Race-guard: if the filesystem mutated between walk and Enter,
    // refuse to switch into a path that is no longer a directory.
    path.is_dir().then_some(path)
}

fn push_pane_system_alert(state: &mut AppState, text: String) {
    let pane = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .cloned();
    let Some(pane) = pane else { return };
    let agent_id = if !pane.agent_id.is_empty() {
        Some(pane.agent_id.clone())
    } else {
        pane.selected_agent_id.clone()
    };
    let at = format!("t+{}", state.metrics.frame_count);
    state.agents.messages.push(AgentMessage {
        at,
        channel: AgentChannel::Agent,
        agent_id,
        mission_id: pane.mission_id.clone(),
        text,
        prompt_msg_idx: None,
        kind: Some(SYSTEM_ALERT_KIND.into()),
    });
}

fn apply_dir_search_event(state: &mut AppState, event: DirSearchEvent) {
    let DirSearchEvent::Results {
        request_id,
        base,
        results,
    } = event;
    let Some(mp) = state.multipane.as_mut() else {
        return;
    };
    let target = mp.panes.iter_mut().find(|p| {
        p.dir_search
            .as_ref()
            .map(|ds| ds.generation == request_id && ds.base == base)
            .unwrap_or(false)
    });
    let Some(pane) = target else { return };
    let Some(ds) = pane.dir_search.as_mut() else {
        return;
    };
    let last = results.len().saturating_sub(1);
    ds.results = results;
    ds.selected = ds.selected.min(last);
    ds.view_offset = 0;
}

fn focused_pane_agent_id(state: &AppState) -> Option<String> {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .and_then(|p| {
            if !p.agent_id.is_empty() {
                Some(p.agent_id.clone())
            } else {
                p.selected_agent_id.clone()
            }
        })
}

/// Pin the focused lane's `current_mission` to the pane's synthetic chat
/// id when no real swarm overlay exists. Locks the breather-filter
/// invariant `agent.current_mission == Some(mission_ctx)` for the render
/// alias even before `dispatch_agent_prompt` rewrites it on dispatch —
/// without this, a stale id from a prior swarm could survive long enough
/// to leak into another pane's render.
fn pin_pane_chat_mission_on_lane(state: &mut AppState, pane_idx: usize) {
    let synthetic = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(pane_idx))
        .filter(|p| p.mission_id.is_none() && !p.chat_mission_id.is_empty())
        .map(|p| p.chat_mission_id.clone());
    let (Some(mid), Some(agent_id)) = (synthetic, focused_pane_agent_id(state)) else {
        return;
    };
    if let Some(lane) = state.agents.agents_get_mut(&agent_id) {
        lane.current_mission = Some(mid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nit_core::{
        AgentLane, AgentLaneKind, AgentStatus, AgentsState, MissionRecord, MultipaneState,
        PaneSession,
    };
    use std::path::PathBuf;
    use std::time::Instant;

    fn fixture_state_no_backend() -> AppState {
        let buffer = nit_core::Buffer::empty("scratch", None);
        let notes = nit_core::Buffer::empty("notes", None);
        let mut state = AppState::new(PathBuf::from("/workspace"), buffer, notes);
        state.agents = AgentsState::default();
        state.agents.agents.push(AgentLane {
            id: "claude-haiku-4-5".into(),
            role: "claude-haiku-4-5".into(),
            lane: "Claude".into(),
            kind: AgentLaneKind::Claude,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
            shadow: false,
        });
        state.agents.agents.push(AgentLane {
            id: "gpt-5".into(),
            role: "gpt-5".into(),
            lane: "Codex".into(),
            kind: AgentLaneKind::Codex,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
            shadow: false,
        });
        state.multipane = Some(MultipaneState {
            backend_agent_id: String::new(),
            panes: vec![
                PaneSession {
                    pane_id: 0,
                    cwd: PathBuf::from("/p0"),
                    ..PaneSession::default()
                },
                PaneSession {
                    pane_id: 1,
                    cwd: PathBuf::from("/p1"),
                    ..PaneSession::default()
                },
            ],
            focused: 0,
            grid_cols: 2,
            grid_rows: 1,
            backend_filter: None,
            help_open: false,
        });
        state
    }

    #[test]
    fn parse_abort_command_recognises_forms() {
        assert_eq!(parse_abort_command("/abort"), Some(AbortScope::Current));
        assert_eq!(parse_abort_command("@abort"), Some(AbortScope::Current));
        assert_eq!(parse_abort_command("/abort all"), Some(AbortScope::All));
        assert_eq!(parse_abort_command("/abort  ALL"), Some(AbortScope::All));
        assert_eq!(
            parse_abort_command("/abort claude#mp-pane-02"),
            Some(AbortScope::Agent("claude#mp-pane-02".into()))
        );
    }

    #[test]
    fn parse_abort_command_rejects_substring_match() {
        assert_eq!(parse_abort_command("/abortif"), None);
        assert_eq!(parse_abort_command("just a regular prompt"), None);
    }

    #[test]
    fn focused_pane_in_roster_mode_when_no_selection() {
        let state = fixture_state_no_backend();
        assert!(focused_pane_in_roster_mode(&state));
    }

    #[test]
    fn move_roster_cursor_clamps_to_visible_lanes() {
        let mut state = fixture_state_no_backend();
        // Two non-shadow lanes => cursor in [0, 1]
        move_roster_cursor(&mut state, 5);
        let cursor = state.multipane.as_ref().unwrap().panes[0].roster_cursor;
        assert_eq!(cursor, 1);
        move_roster_cursor(&mut state, -10);
        let cursor = state.multipane.as_ref().unwrap().panes[0].roster_cursor;
        assert_eq!(cursor, 0);
    }

    #[test]
    fn revert_focused_pane_to_roster_clears_selection() {
        let mut state = fixture_state_no_backend();
        if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(0) {
            pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-00".into());
            pane.agent_id = "claude-haiku-4-5#mp-pane-00".into();
            pane.chat_input = "buffered".into();
        }
        revert_focused_pane_to_roster(&mut state);
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert!(pane.selected_agent_id.is_none());
        assert!(pane.agent_id.is_empty());
        assert!(pane.chat_input.is_empty());
    }

    #[test]
    fn abort_with_no_selection_emits_system_message() {
        let mut state = fixture_state_no_backend();
        let before = state.agents.messages.len();
        // No selection in either pane → the focused-pane abort
        // shortcut posts a "nothing to abort" notice without invoking
        // the runner-bound `handle_abort` (which would need real
        // CodexRunner / ClaudeRunner stubs).
        assert!(focused_pane_agent_id(&state).is_none());
        push_pane_system_message(&mut state, "no agent selected — nothing to abort".into());
        assert_eq!(state.agents.messages.len(), before + 1);
        assert!(state
            .agents
            .messages
            .last()
            .unwrap()
            .text
            .contains("no agent selected"));
    }

    fn fixture_with_efforts() -> AppState {
        let mut state = fixture_state_no_backend();
        state.agents.codex_supported_reasoning_efforts.insert(
            "gpt-5".into(),
            vec!["low".into(), "medium".into(), "high".into()],
        );
        state
            .agents
            .claude_supported_efforts
            .insert("claude-haiku-4-5".into(), vec!["low".into(), "max".into()]);
        state
    }

    #[test]
    fn expand_at_cursor_expands_focused_backend() {
        let mut state = fixture_with_efforts();
        // Cursor lands on the first selectable row (Backend Codex).
        // Pressing `l` drills to its first child, which auto-latches
        // the parent backend through sync_auto_expansion.
        move_roster_cursor(&mut state, 0);
        expand_at_cursor(&mut state);
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert_eq!(pane.auto_expanded_backend, Some(AgentLaneKind::Codex));

        // Pressing `h` from the child drills the cursor back up; the
        // backend latch clears as soon as the cursor leaves the group.
        collapse_at_cursor(&mut state);
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert_eq!(pane.auto_expanded_backend, Some(AgentLaneKind::Codex));
        // Two `h`s in a row land back on the Backend Codex row, then
        // pressing `h` again is a no-op (no row above).
    }

    #[test]
    fn cursor_walk_skips_size_leaves() {
        let mut state = fixture_with_efforts();
        // Cursor starts on Backend Codex (auto-latches the group);
        // walking once moves to Agent gpt-5; walking again hops over
        // every SizeBranch / SizeLeaf row and lands on Backend Claude.
        move_roster_cursor(&mut state, 0);
        assert_eq!(state.multipane.as_ref().unwrap().panes[0].roster_cursor, 0);
        move_roster_cursor(&mut state, 1);
        assert_eq!(state.multipane.as_ref().unwrap().panes[0].roster_cursor, 1);
        move_roster_cursor(&mut state, 1);
        assert_eq!(state.multipane.as_ref().unwrap().panes[0].roster_cursor, 2);
        // The cursor sits on a Backend, so roster_tree_selected stays None.
        assert!(state.multipane.as_ref().unwrap().panes[0]
            .roster_tree_selected
            .is_none());
    }

    #[test]
    fn auto_expand_on_cursor_move_to_backend() {
        let mut state = fixture_with_efforts();
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert_eq!(pane.roster_cursor, 0, "starts on first selectable");
        // Trigger auto-expansion by re-seating the cursor at 0 via a 0-delta move.
        move_roster_cursor(&mut state, 0);
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert_eq!(pane.auto_expanded_backend, Some(AgentLaneKind::Codex));
        assert!(pane.auto_expanded_agent.is_none());
    }

    #[test]
    fn auto_collapse_on_cursor_leave() {
        let mut state = fixture_with_efforts();
        // Land on Backend Codex → auto_expanded_backend = Codex.
        move_roster_cursor(&mut state, 0);
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert_eq!(pane.auto_expanded_backend, Some(AgentLaneKind::Codex));
        // Walk down to the next selectable row — Agent under Codex
        // (because compute_rows now considers auto_expanded_backend).
        move_roster_cursor(&mut state, 1);
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        // After moving onto the Agent row, auto_expanded_agent latches
        // and the auto-expanded backend stays set.
        assert_eq!(pane.auto_expanded_backend, Some(AgentLaneKind::Codex));
        assert_eq!(pane.auto_expanded_agent.as_deref(), Some("gpt-5"));
        // Walk to next selectable — leaves the Codex group entirely
        // (cursor lands on Backend Claude). Codex auto-fields collapse.
        move_roster_cursor(&mut state, 1);
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert_eq!(pane.auto_expanded_backend, Some(AgentLaneKind::Claude));
        assert!(pane.auto_expanded_agent.is_none());
    }

    #[test]
    fn size_leaf_click_writes_codex_selected_effort() {
        let mut state = fixture_with_efforts();
        // Manually drive size selection via the click path — the cursor
        // never stops on size rows under the new gating.
        let mut pane_clone = state.multipane.as_ref().unwrap().panes[0].clone();
        pane_clone.auto_expanded_backend = Some(AgentLaneKind::Codex);
        pane_clone.auto_expanded_agent = Some("gpt-5".into());
        let rows = roster_view::compute_rows(&state, &pane_clone, None);
        let target_idx = rows
            .iter()
            .position(|r| matches!(r, roster_view::PaneRosterRow::SizeLeaf { effort, .. } if effort == "medium"))
            .expect("medium leaf");
        let leaf_row = rows[target_idx].clone();
        apply_roster_click(
            &mut state,
            RosterClickTarget {
                pane_idx: 0,
                rows,
                row_idx: target_idx,
                row: leaf_row,
                local_x: 12,
            },
        );
        assert_eq!(
            state.multipane.as_ref().unwrap().panes[0]
                .selected_effort
                .get("gpt-5"),
            Some(&"medium".to_string())
        );
    }

    #[test]
    fn clicking_two_backends_only_expands_the_second() {
        // Regression for the operator-reported bug: clicking Backend
        // Codex then Backend Claude must NOT leave Codex expanded.
        // Under the cursor-only model, only the most recently clicked
        // backend is expanded.
        let mut state = fixture_with_efforts();
        let pane = state.multipane.as_ref().unwrap().panes[0].clone();
        let rows = roster_view::compute_rows(&state, &pane, None);
        let codex_idx = rows
            .iter()
            .position(|r| {
                matches!(
                    r,
                    roster_view::PaneRosterRow::Backend {
                        kind: AgentLaneKind::Codex
                    }
                )
            })
            .expect("codex backend row");
        let codex_row = rows[codex_idx].clone();
        apply_roster_click(
            &mut state,
            RosterClickTarget {
                pane_idx: 0,
                rows: rows.clone(),
                row_idx: codex_idx,
                row: codex_row,
                local_x: 1,
            },
        );
        let pane = state.multipane.as_ref().unwrap().panes[0].clone();
        let rows = roster_view::compute_rows(&state, &pane, None);
        let claude_idx = rows
            .iter()
            .position(|r| {
                matches!(
                    r,
                    roster_view::PaneRosterRow::Backend {
                        kind: AgentLaneKind::Claude
                    }
                )
            })
            .expect("claude backend row");
        let claude_row = rows[claude_idx].clone();
        apply_roster_click(
            &mut state,
            RosterClickTarget {
                pane_idx: 0,
                rows,
                row_idx: claude_idx,
                row: claude_row,
                local_x: 1,
            },
        );
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert_eq!(pane.auto_expanded_backend, Some(AgentLaneKind::Claude));
        let rows = roster_view::compute_rows(&state, pane, None);
        let codex_visible = rows.iter().any(|r| {
            matches!(
                r,
                roster_view::PaneRosterRow::Agent {
                    kind: AgentLaneKind::Codex,
                    ..
                }
            )
        });
        let claude_visible = rows.iter().any(|r| {
            matches!(
                r,
                roster_view::PaneRosterRow::Agent {
                    kind: AgentLaneKind::Claude,
                    ..
                }
            )
        });
        assert!(
            claude_visible,
            "Claude group expanded after the second click"
        );
        assert!(
            !codex_visible,
            "Codex group must collapse when click moves to Claude"
        );
    }

    #[test]
    fn cursor_clamps_after_h_collapse() {
        // After h on a child row drills the cursor up off the backend,
        // the cursor index stays in [0, selectable_count).
        let mut state = fixture_with_efforts();
        // Land on Backend Codex, drill into gpt-5, then collapse — the
        // cursor must clamp to the new selectable range.
        move_roster_cursor(&mut state, 0);
        expand_at_cursor(&mut state); // cursor → gpt-5 (idx 1)
        assert_eq!(state.multipane.as_ref().unwrap().panes[0].roster_cursor, 1);
        collapse_at_cursor(&mut state); // cursor drills back up to idx 0
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        let rows = roster_view::compute_rows(&state, pane, None);
        let stops = roster_view::selectable_count(&rows);
        assert!(pane.roster_cursor < stops);
    }

    #[test]
    fn revert_to_roster_clears_auto_fields_and_dir_search() {
        let mut state = fixture_state_no_backend();
        if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(0) {
            pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-00".into());
            pane.agent_id = "claude-haiku-4-5#mp-pane-00".into();
            pane.auto_expanded_backend = Some(AgentLaneKind::Claude);
            pane.auto_expanded_agent = Some("claude-haiku-4-5".into());
            pane.dir_search = Some(nit_core::DirSearchState {
                query: "abc".into(),
                ..Default::default()
            });
        }
        revert_focused_pane_to_roster(&mut state);
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert!(pane.auto_expanded_backend.is_none());
        assert!(pane.auto_expanded_agent.is_none());
        assert!(pane.dir_search.is_none());
    }

    #[test]
    fn jump_roster_cursor_to_top_resets_scroll() {
        let mut state = fixture_with_efforts();
        if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(0) {
            pane.roster_cursor = 1;
            pane.roster_scroll = 5;
        }
        jump_roster_cursor_to_top(&mut state);
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert_eq!(pane.roster_cursor, 0);
        assert_eq!(pane.roster_scroll, 0);
    }

    #[test]
    fn jump_roster_cursor_to_bottom_lands_on_last_selectable() {
        let mut state = fixture_with_efforts();
        jump_roster_cursor_to_bottom(&mut state);
        let cursor = state.multipane.as_ref().unwrap().panes[0].roster_cursor;
        // Two backends collapsed → 2 selectable rows.
        assert_eq!(cursor, 1);
    }

    #[test]
    fn scroll_chat_thread_clamps_at_zero() {
        let mut state = fixture_state_no_backend();
        if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(0) {
            pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-00".into());
            pane.agent_id = "claude-haiku-4-5#mp-pane-00".into();
        }
        let swarm = SwarmRuntime::default();
        let area = Rect::new(0, 0, 80, 30);
        // Bare fixture has no rendered messages → max_scroll = 0. PgUp
        // (delta < 0) from the top must NOT re-engage the follow-bottom
        // sentinel — only wheel-down past the bottom does. Otherwise
        // operator scroll-up gestures would silently snap back to BOTTOM
        // every time max_scroll transiently equals current scroll (the
        // BUG-3 root cause).
        let mp = state.multipane.as_mut().unwrap();
        if let Some(pane) = mp.panes.get_mut(0) {
            pane.chat_thread_scroll = 0;
        }
        scroll_chat_thread(&mut state, &swarm, area, -3);
        assert_eq!(
            state.multipane.as_ref().unwrap().panes[0].chat_thread_scroll,
            0,
            "PgUp from row 0 must stay at row 0, not silently snap to BOTTOM"
        );
    }

    #[test]
    fn handle_mouse_scroll_clamps_at_max_with_short_thread() {
        let mut state = fixture_state_no_backend();
        if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(1) {
            pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-01".into());
            pane.agent_id = "claude-haiku-4-5#mp-pane-01".into();
            pane.chat_thread_scroll = 999; // pretend a runaway scroll
        }
        let swarm = SwarmRuntime::default();
        let area = Rect::new(0, 0, 80, 30);
        let pane1_rect = grid::pane_rect(area, 2, 1, 1);
        // Wheel down on a short thread MUST clear the runaway 999 +
        // delta. With the "stick to bottom" sentinel semantics, the
        // resolved scroll lands at max_scroll (==0 here, no messages)
        // and `next >= max_scroll` re-engages the sentinel — exactly
        // what the operator wants for "follow new content".
        handle_mouse_scroll(
            &mut state,
            &swarm,
            area,
            pane1_rect.x + 5,
            pane1_rect.y + 5,
            3,
        );
        assert_eq!(
            state.multipane.as_ref().unwrap().panes[1].chat_thread_scroll,
            nit_core::CONSOLE_SCROLL_BOTTOM,
            "wheel-down past max_scroll must re-engage the follow-bottom sentinel"
        );
    }

    #[test]
    fn handle_mouse_scroll_targets_roster_or_chat_per_pane_mode() {
        let mut state = fixture_state_no_backend();
        // Pane 0 stays in roster mode; pane 1 becomes a chat pane.
        if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(1) {
            pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-01".into());
            pane.agent_id = "claude-haiku-4-5#mp-pane-01".into();
        }
        let swarm = SwarmRuntime::default();
        let area = Rect::new(0, 0, 80, 30);
        let pane0_rect = grid::pane_rect(area, 2, 1, 0);
        let pane1_rect = grid::pane_rect(area, 2, 1, 1);

        // Wheel down inside pane 0 → roster_scroll bumps (clamped to
        // roster row count).
        handle_mouse_scroll(
            &mut state,
            &swarm,
            area,
            pane0_rect.x + 5,
            pane0_rect.y + 5,
            1,
        );
        let roster = state.multipane.as_ref().unwrap().panes[0].roster_scroll;
        assert!(
            roster <= 1,
            "roster_scroll should bump or clamp, got {roster}"
        );

        // Wheel down inside pane 1 (chat mode, empty thread) — re-engages
        // the follow-bottom sentinel because next >= max_scroll(=0).
        handle_mouse_scroll(
            &mut state,
            &swarm,
            area,
            pane1_rect.x + 5,
            pane1_rect.y + 5,
            1,
        );
        assert_eq!(
            state.multipane.as_ref().unwrap().panes[1].chat_thread_scroll,
            nit_core::CONSOLE_SCROLL_BOTTOM,
        );
    }

    #[test]
    fn template_click_writes_to_focused_pane_only() {
        let mut state = fixture_state_no_backend();
        let pane = state.multipane.as_ref().unwrap().panes[0].clone();
        let rows = roster_view::compute_rows(&state, &pane, None);
        // Click on the " parallel " word (after " lab " + 1 separator).
        let parallel_col = " Template: ".chars().count() + " lab ".chars().count() + 1 + 1;
        apply_roster_click(
            &mut state,
            RosterClickTarget {
                pane_idx: 0,
                rows,
                row_idx: 0,
                row: roster_view::PaneRosterRow::Template,
                local_x: parallel_col,
            },
        );
        assert_eq!(
            state.multipane.as_ref().unwrap().panes[0].swarm_template,
            "parallel"
        );
        assert_eq!(
            state.agents.swarm_default_template, "lab",
            "global default must stay untouched by per-pane clicks"
        );
    }

    #[test]
    fn template_click_on_pane_zero_does_not_touch_pane_one() {
        let mut state = fixture_state_no_backend();
        let pane = state.multipane.as_ref().unwrap().panes[0].clone();
        let rows = roster_view::compute_rows(&state, &pane, None);
        let parallel_col = " Template: ".chars().count() + " lab ".chars().count() + 1 + 1;
        apply_roster_click(
            &mut state,
            RosterClickTarget {
                pane_idx: 0,
                rows,
                row_idx: 0,
                row: roster_view::PaneRosterRow::Template,
                local_x: parallel_col,
            },
        );
        let panes = &state.multipane.as_ref().unwrap().panes;
        assert_eq!(panes[0].swarm_template, "parallel");
        assert_eq!(
            panes[1].swarm_template, "lab",
            "sibling pane's template must not change"
        );
    }

    #[test]
    fn mission_click_writes_to_focused_pane_only() {
        let mut state = fixture_state_no_backend();
        let pane = state.multipane.as_ref().unwrap().panes[0].clone();
        let rows = roster_view::compute_rows(&state, &pane, None);
        let general_col = " Mission:  ".chars().count() + " auto ".chars().count() + 1 + 1;
        apply_roster_click(
            &mut state,
            RosterClickTarget {
                pane_idx: 0,
                rows,
                row_idx: 1,
                row: roster_view::PaneRosterRow::Mission,
                local_x: general_col,
            },
        );
        let panes = &state.multipane.as_ref().unwrap().panes;
        assert_eq!(panes[0].swarm_mission, "general");
        assert_eq!(
            panes[1].swarm_mission, "auto",
            "sibling pane's mission must not change"
        );
        assert_eq!(
            state.agents.swarm_default_mission, "auto",
            "global default must stay untouched"
        );
    }

    #[test]
    fn fresh_pane_first_render_no_artifact_callout() {
        // Documentary regression for the screenshot bug: a freshly-
        // selected pane (no dispatch yet) must keep `has_run_mission`
        // false, which the renderer uses to suppress artifact callouts.
        let state = fixture_state_no_backend();
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert!(!pane.has_run_mission);
    }

    #[test]
    fn dispatch_sets_has_run_mission_true() {
        let mut state = fixture_state_no_backend();
        if let Some(p) = state.multipane.as_mut().unwrap().panes.get_mut(0) {
            p.selected_agent_id = Some("claude-haiku-4-5#mp-pane-00".into());
            p.agent_id = "claude-haiku-4-5#mp-pane-00".into();
        }
        let mut vitals = VitalsState::default();
        let outcome = crate::multipane::dispatch::dispatch_pane_prompt(
            &mut state,
            &mut vitals,
            None,
            None,
            0,
            "ping".into(),
        );
        assert_eq!(
            outcome,
            crate::multipane::dispatch::DispatchOutcome::Dispatched
        );
        assert!(state.multipane.as_ref().unwrap().panes[0].has_run_mission);
    }

    fn open_dir_search_with_results(state: &mut AppState, results: Vec<PathBuf>) {
        if let Some(pane) = state.multipane.as_mut().and_then(|mp| mp.panes.get_mut(0)) {
            pane.dir_search = Some(nit_core::DirSearchState {
                results,
                base: PathBuf::from("/tmp"),
                generation: 1,
                ..Default::default()
            });
        }
    }

    #[test]
    fn focused_pane_dir_search_active_reflects_overlay() {
        let mut state = fixture_state_no_backend();
        assert!(!focused_pane_dir_search_active(&state));
        open_dir_search_with_results(&mut state, Vec::new());
        assert!(focused_pane_dir_search_active(&state));
    }

    #[test]
    fn close_focused_dir_search_drops_overlay() {
        let mut state = fixture_state_no_backend();
        open_dir_search_with_results(&mut state, Vec::new());
        close_focused_dir_search(&mut state);
        assert!(state.multipane.as_ref().unwrap().panes[0]
            .dir_search
            .is_none());
    }

    #[test]
    fn commit_dir_search_with_empty_results_is_noop() {
        let mut state = fixture_state_no_backend();
        let cwd_before = state.multipane.as_ref().unwrap().panes[0].cwd.clone();
        open_dir_search_with_results(&mut state, Vec::new());
        commit_dir_search(&mut state);
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert_eq!(pane.cwd, cwd_before);
        assert!(pane.dir_search.is_none());
    }

    #[test]
    fn commit_dir_search_changes_cwd_and_emits_system_alert() {
        let mut state = fixture_state_no_backend();
        let tmp = std::env::temp_dir().join(format!(
            "nit-mp-commit-{}",
            Instant::now().elapsed().as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        open_dir_search_with_results(&mut state, vec![tmp.clone()]);
        let before = state.agents.messages.len();
        commit_dir_search(&mut state);
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert_eq!(pane.cwd, tmp);
        assert!(pane.dir_search.is_none());
        assert_eq!(state.agents.messages.len(), before + 1);
        let last = state.agents.messages.last().unwrap();
        assert_eq!(last.kind.as_deref(), Some(SYSTEM_ALERT_KIND));
        assert!(last.text.starts_with("cwd → "), "text was {:?}", last.text);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn commit_dir_search_rejects_path_that_is_not_a_dir() {
        let mut state = fixture_state_no_backend();
        let cwd_before = state.multipane.as_ref().unwrap().panes[0].cwd.clone();
        open_dir_search_with_results(
            &mut state,
            vec![PathBuf::from("/this/path/does/not/exist/abc")],
        );
        commit_dir_search(&mut state);
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert_eq!(pane.cwd, cwd_before);
        assert!(pane.dir_search.is_none());
    }

    // Lens-E Part C: switching cwd drops focused-pane resume ids so the
    // next turn opens a fresh session in the new cwd. Without this,
    // session metadata re-anchors to the original workspace.
    #[test]
    fn commit_dir_search_invalidates_resume_session_ids_for_focused_pane() {
        let mut state = fixture_state_no_backend();
        let lane = "claude-haiku-4-5#mp-pane-00";
        let mission = "mission-XYZ";
        if let Some(pane) = focused_pane_mut(&mut state) {
            pane.agent_id = lane.into();
            pane.mission_id = Some(mission.into());
        }
        state
            .agents
            .claude_session_ids
            .insert(lane.into(), "A".into());
        state
            .agents
            .codex_thread_ids
            .insert(lane.into(), "B".into());
        state
            .agents
            .claude_mission_session_ids
            .entry(mission.into())
            .or_default()
            .insert(lane.into(), "C".into());
        state
            .agents
            .codex_mission_thread_ids
            .entry(mission.into())
            .or_default()
            .insert(lane.into(), "D".into());

        let tmp = std::env::temp_dir().join(format!(
            "nit-resume-inval-{}",
            Instant::now().elapsed().as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        open_dir_search_with_results(&mut state, vec![tmp.clone()]);
        commit_dir_search(&mut state);

        let agents = &state.agents;
        assert!(!agents.claude_session_ids.contains_key(lane));
        assert!(!agents.codex_thread_ids.contains_key(lane));
        assert!(agents
            .claude_mission_session_ids
            .get(mission)
            .is_none_or(|inner| !inner.contains_key(lane)));
        assert!(agents
            .codex_mission_thread_ids
            .get(mission)
            .is_none_or(|inner| !inner.contains_key(lane)));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Selecting a directory in pane 0 must not mutate pane 1's cwd or
    // invalidate pane 1's resume sessions.
    #[test]
    fn commit_dir_search_in_pane0_does_not_affect_pane1() {
        let mut state = fixture_state_no_backend();
        let other = "claude-haiku-4-5#mp-pane-01";
        if let Some(mp) = state.multipane.as_mut() {
            mp.panes[0].agent_id = "claude-haiku-4-5#mp-pane-00".into();
            mp.panes[1].agent_id = other.into();
        }
        state
            .agents
            .claude_session_ids
            .insert(other.into(), "stay".into());
        let pane1_cwd = state.multipane.as_ref().unwrap().panes[1].cwd.clone();

        let tmp = std::env::temp_dir().join(format!(
            "nit-pane-iso-{}",
            Instant::now().elapsed().as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        open_dir_search_with_results(&mut state, vec![tmp.clone()]);
        commit_dir_search(&mut state);

        let mp = state.multipane.as_ref().unwrap();
        assert_eq!(mp.panes[0].cwd, tmp);
        assert_eq!(mp.panes[1].cwd, pane1_cwd);
        assert_eq!(
            state
                .agents
                .claude_session_ids
                .get(other)
                .map(String::as_str),
            Some("stay"),
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn apply_dir_search_event_writes_results_when_generation_matches() {
        let mut state = fixture_state_no_backend();
        let base = PathBuf::from("/tmp");
        if let Some(pane) = state.multipane.as_mut().and_then(|mp| mp.panes.get_mut(0)) {
            pane.dir_search = Some(nit_core::DirSearchState {
                query: "foo".into(),
                query_cursor: 3,
                base: base.clone(),
                generation: 7,
                ..Default::default()
            });
        }
        let want = vec![PathBuf::from("/tmp/alpha")];
        apply_dir_search_event(
            &mut state,
            DirSearchEvent::Results {
                request_id: 7,
                base: base.clone(),
                results: want.clone(),
            },
        );
        let ds = state.multipane.as_ref().unwrap().panes[0]
            .dir_search
            .as_ref()
            .unwrap();
        assert_eq!(ds.results, want);
    }

    fn open_dir_search_with(state: &mut AppState, results: Vec<PathBuf>, last_visible: u16) {
        if let Some(pane) = state.multipane.as_mut().and_then(|mp| mp.panes.get_mut(0)) {
            pane.dir_search = Some(nit_core::DirSearchState {
                results,
                base: PathBuf::from("/tmp"),
                generation: 1,
                last_visible,
                ..Default::default()
            });
        }
    }

    #[test]
    fn ctrl_j_advances_dir_search_selection() {
        let mut state = fixture_state_no_backend();
        let results = vec![
            PathBuf::from("/tmp/a"),
            PathBuf::from("/tmp/b"),
            PathBuf::from("/tmp/c"),
        ];
        open_dir_search_with(&mut state, results, 10);
        with_focused_dir_search(&mut state, move_selected_down);
        let ds = state.multipane.as_ref().unwrap().panes[0]
            .dir_search
            .as_ref()
            .unwrap();
        assert_eq!(ds.selected, 1);
    }

    #[test]
    fn ctrl_k_recedes_dir_search_selection() {
        let mut state = fixture_state_no_backend();
        let results = vec![
            PathBuf::from("/tmp/a"),
            PathBuf::from("/tmp/b"),
            PathBuf::from("/tmp/c"),
        ];
        open_dir_search_with(&mut state, results, 10);
        with_focused_dir_search(&mut state, |ds| ds.selected = 2);
        with_focused_dir_search(&mut state, move_selected_up);
        let ds = state.multipane.as_ref().unwrap().panes[0]
            .dir_search
            .as_ref()
            .unwrap();
        assert_eq!(ds.selected, 1);
    }

    #[test]
    fn ctrl_l_expands_focused_dir() {
        let tmp = std::env::temp_dir().join(format!(
            "nit-mp-expand-{}",
            Instant::now().elapsed().as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let mut state = fixture_state_no_backend();
        open_dir_search_with(&mut state, vec![tmp.clone()], 10);
        expand_dir_search_at_cursor(&mut state);
        let ds = state.multipane.as_ref().unwrap().panes[0]
            .dir_search
            .as_ref()
            .unwrap();
        assert!(ds.expanded.contains(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ctrl_h_collapses_focused_dir() {
        let tmp = std::env::temp_dir().join(format!(
            "nit-mp-collapse-{}",
            Instant::now().elapsed().as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let mut state = fixture_state_no_backend();
        open_dir_search_with(&mut state, vec![tmp.clone()], 10);
        with_focused_dir_search(&mut state, |ds| {
            ds.expanded.insert(tmp.clone());
        });
        collapse_dir_search_at_cursor(&mut state);
        let ds = state.multipane.as_ref().unwrap().panes[0]
            .dir_search
            .as_ref()
            .unwrap();
        assert!(!ds.expanded.contains(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn view_offset_advances_when_selected_passes_window() {
        let mut state = fixture_state_no_backend();
        let results: Vec<PathBuf> = (0..25)
            .map(|i| PathBuf::from(format!("/tmp/d{i}")))
            .collect();
        open_dir_search_with(&mut state, results, 10);
        for _ in 0..12 {
            with_focused_dir_search(&mut state, move_selected_down);
        }
        let ds = state.multipane.as_ref().unwrap().panes[0]
            .dir_search
            .as_ref()
            .unwrap();
        assert_eq!(ds.selected, 12);
        assert_eq!(ds.view_offset, 3);
    }

    #[test]
    fn view_offset_recedes_when_selected_above_window() {
        let mut state = fixture_state_no_backend();
        let results: Vec<PathBuf> = (0..25)
            .map(|i| PathBuf::from(format!("/tmp/d{i}")))
            .collect();
        open_dir_search_with(&mut state, results, 10);
        with_focused_dir_search(&mut state, |ds| {
            ds.selected = 11;
            ds.view_offset = 10;
        });
        for _ in 0..7 {
            with_focused_dir_search(&mut state, move_selected_up);
        }
        let ds = state.multipane.as_ref().unwrap().panes[0]
            .dir_search
            .as_ref()
            .unwrap();
        assert_eq!(ds.selected, 4);
        assert_eq!(ds.view_offset, 4);
    }

    #[test]
    fn apply_dir_search_event_resets_view_offset() {
        let mut state = fixture_state_no_backend();
        let base = PathBuf::from("/tmp");
        if let Some(pane) = state.multipane.as_mut().and_then(|mp| mp.panes.get_mut(0)) {
            pane.dir_search = Some(nit_core::DirSearchState {
                base: base.clone(),
                generation: 7,
                view_offset: 12,
                ..Default::default()
            });
        }
        apply_dir_search_event(
            &mut state,
            DirSearchEvent::Results {
                request_id: 7,
                base,
                results: vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")],
            },
        );
        let ds = state.multipane.as_ref().unwrap().panes[0]
            .dir_search
            .as_ref()
            .unwrap();
        assert_eq!(ds.view_offset, 0);
    }

    #[test]
    fn compute_dropdown_rows_clamps_to_results_len() {
        assert_eq!(compute_dropdown_rows(40, 2), 2);
        assert_eq!(compute_dropdown_rows(40, 0), 1);
    }

    #[test]
    fn compute_dropdown_rows_min_three_max_sixteen() {
        assert_eq!(compute_dropdown_rows(2, 30), 3);
        assert_eq!(compute_dropdown_rows(200, 30), 16);
    }

    #[test]
    fn apply_dir_search_event_drops_results_for_stale_generation() {
        let mut state = fixture_state_no_backend();
        let base = PathBuf::from("/tmp");
        if let Some(pane) = state.multipane.as_mut().and_then(|mp| mp.panes.get_mut(0)) {
            pane.dir_search = Some(nit_core::DirSearchState {
                query: "foo".into(),
                query_cursor: 3,
                base: base.clone(),
                generation: 7,
                ..Default::default()
            });
        }
        apply_dir_search_event(
            &mut state,
            DirSearchEvent::Results {
                request_id: 6,
                base,
                results: vec![PathBuf::from("/tmp/old")],
            },
        );
        let ds = state.multipane.as_ref().unwrap().panes[0]
            .dir_search
            .as_ref()
            .unwrap();
        assert!(ds.results.is_empty());
    }

    /// Bug 2: when the dir-search dropdown is open, clicks below the
    /// overlay must resolve to the row visually under the cursor — not
    /// `DIR_SEARCH_INPUT_ROWS + visible_rows` rows above it.
    #[test]
    fn roster_click_with_dir_search_open_hits_correct_row() {
        let mut state = fixture_state_no_backend();
        let area = Rect::new(0, 0, 80, 30);
        // Single pane fills the grid, focused.
        if let Some(mp) = state.multipane.as_mut() {
            mp.panes.truncate(1);
            mp.grid_cols = 1;
            mp.grid_rows = 1;
            mp.focused = 0;
        }
        // Open dir-search with three results so the overlay reserves
        // `DIR_SEARCH_INPUT_ROWS + 3` rows of header at the top of the
        // pane's inner area.
        let results = vec![
            PathBuf::from("/tmp/a"),
            PathBuf::from("/tmp/b"),
            PathBuf::from("/tmp/c"),
        ];
        if let Some(pane) = state.multipane.as_mut().and_then(|mp| mp.panes.get_mut(0)) {
            pane.dir_search = Some(nit_core::DirSearchState {
                results,
                base: PathBuf::from("/tmp"),
                generation: 1,
                ..Default::default()
            });
        }
        let pane = state.multipane.as_ref().unwrap().panes[0].clone();
        let pane_rect = grid::pane_rect(area, 1, 1, 0);
        let inner = pane_inner_after_chrome(pane_rect);
        let body = dir_search_body_rect(inner, &pane);
        // Click on the first visible roster row WITHIN the body — i.e.
        // the row the operator sees is row 0 of the roster body.
        let click_x = body.x + 1;
        let click_y = body.y;
        let target = resolve_left_click_target(&mut state, area, click_x, click_y);
        let target = target.expect("click should resolve to a roster target");
        // Computed roster rows include the Template / Mission preamble
        // plus backend / agent rows. Whatever the first selectable row
        // is, it must be the one at row_idx 0 — meaning the overlay
        // strip was correctly accounted for.
        assert_eq!(
            target.row_idx, 0,
            "click on the first visible roster row must resolve to row_idx 0, got {}",
            target.row_idx
        );
    }

    /// Bug 2: `handle_mouse` must strip the chrome (top status row +
    /// bottom hint row) before passing coordinates downstream — so a
    /// click on the very first visible pane row routes to a real
    /// roster row instead of falling outside the pane.
    #[test]
    fn roster_click_after_chrome_strip_resolves_visual_row() {
        let mut state = fixture_state_no_backend();
        let area = Rect::new(0, 0, 80, 30);
        if let Some(mp) = state.multipane.as_mut() {
            mp.panes.truncate(1);
            mp.grid_cols = 1;
            mp.grid_rows = 1;
            mp.focused = 0;
        }
        // Click 1 row below the top chrome, which the renderer paints
        // as the start of pane 0. Without chrome-strip, `pane_at_point`
        // would map this to the chrome row and resolve_left_click_target
        // would return None.
        let click_y = area.y + 1;
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: click_y,
            modifiers: KeyModifiers::empty(),
        };
        let swarm = SwarmRuntime::default();
        let theme = crate::theme::Theme::default();
        let mut clipboard: Option<arboard::Clipboard> = None;
        // Just call handle_mouse — no panic / no out-of-bounds means
        // the chrome strip and downstream resolver agreed on the rect.
        handle_mouse(&mut state, &swarm, &theme, &mut clipboard, area, mouse);
        // The pane should still exist and not have been corrupted.
        assert_eq!(state.multipane.as_ref().unwrap().panes.len(), 1);
    }

    /// Bug 3: clicking an artifact line in pane B must scope the popup
    /// resolver to pane B's mission/agent — not whichever values the
    /// last-rendered pane left in `state.agents.selected_*`.
    #[test]
    fn try_open_chat_pane_artifact_uses_pane_context_and_swarm() {
        let mut state = fixture_state_no_backend();
        // Two panes, two missions, two agents — pane 0 belongs to
        // mission A and agent A, pane 1 belongs to mission B and agent B.
        state.agents.messages.clear();
        state.agents.missions.clear();
        let now = "t+0".to_string();
        state.agents.missions.push(MissionRecord {
            id: "mis-A".into(),
            title: "A".into(),
            phase: nit_core::MissionPhase::Plan,
            swarm: false,
            assigned_agents: Vec::new(),
            status: "Planning".into(),
            updated_at: now.clone(),
        });
        state.agents.missions.push(MissionRecord {
            id: "mis-B".into(),
            title: "B".into(),
            phase: nit_core::MissionPhase::Plan,
            swarm: false,
            assigned_agents: Vec::new(),
            status: "Planning".into(),
            updated_at: now,
        });
        // Configure pane 0 → mission A, pane 1 → mission B
        if let Some(mp) = state.multipane.as_mut() {
            mp.panes.truncate(2);
            mp.grid_cols = 2;
            mp.grid_rows = 1;
            mp.focused = 0;
            mp.panes[0].selected_agent_id = Some("claude-haiku-4-5#mp-pane-00".into());
            mp.panes[0].agent_id = "claude-haiku-4-5#mp-pane-00".into();
            mp.panes[0].mission_id = Some("mis-A".into());
            mp.panes[0].has_run_mission = true;
            mp.panes[1].selected_agent_id = Some("claude-haiku-4-5#mp-pane-01".into());
            mp.panes[1].agent_id = "claude-haiku-4-5#mp-pane-01".into();
            mp.panes[1].mission_id = Some("mis-B".into());
            mp.panes[1].has_run_mission = true;
        }
        // Leave selected_* pointing at pane A — the way it would look
        // after pane A renders. The buggy resolver would walk pane A's
        // messages even when the click is in pane B.
        state.agents.selected_mission = Some("mis-A".into());
        state.agents.selected_agent = Some("claude-haiku-4-5#mp-pane-00".into());

        let area = Rect::new(0, 0, 80, 30);
        let pane_b_rect = grid::pane_rect(area, 2, 1, 1);
        let swarm = SwarmRuntime::default();
        // Click somewhere inside pane B's thread area. We don't care
        // whether an artifact actually opens (no messages in this
        // fixture) — we care that the click does NOT corrupt
        // `selected_mission` to pane A's value, since the resolver
        // has been wrapped to alias to pane B and restore on miss.
        let opened = try_open_chat_pane_artifact(
            &mut state,
            &swarm,
            area,
            pane_b_rect.x + 2,
            pane_b_rect.y + 2,
        );
        assert!(
            !opened,
            "no artifact rows in fixture, click must miss cleanly"
        );
        // On miss, the alias is restored, so selected_* point back at
        // pane A's values (matching what they were when we entered).
        assert_eq!(
            state.agents.selected_mission.as_deref(),
            Some("mis-A"),
            "selected_mission must be restored to its prior value on miss"
        );
        assert_eq!(
            state.agents.selected_agent.as_deref(),
            Some("claude-haiku-4-5#mp-pane-00"),
            "selected_agent must be restored to its prior value on miss"
        );
    }

    // ----- BUG 3: scroll holds when row count oscillates -------------------
    //
    // When swarm bus events flip breather rows in/out of the visible
    // window, max_scroll oscillates frame-to-frame. The pre-fix snap-back
    // rule re-engaged the follow-bottom sentinel whenever `next >= max_scroll`
    // — silently consuming PgUp / wheel-up gestures during a swarm. The
    // delta-guard restricts the snap-back to operator-driven scroll-DOWN.

    #[test]
    fn pgup_does_not_re_engage_sentinel_when_max_scroll_dips() {
        // Pane is parked at scroll = 10 with current max_scroll = 30.
        // A swarm event drops the trailing breather count, max_scroll is
        // recomputed at 0 inside `scroll_chat_thread`. Before the fix:
        // `next = 10 - 8 = 2`, `2 >= 0` → snap to BOTTOM. After the fix:
        // delta < 0 so the sentinel stays disengaged, scroll lands at 0
        // (clamped to max_scroll, not BOTTOM).
        let mut state = fixture_state_no_backend();
        if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(0) {
            pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-00".into());
            pane.agent_id = "claude-haiku-4-5#mp-pane-00".into();
            pane.chat_thread_scroll = 10;
        }
        let swarm = SwarmRuntime::default();
        let area = Rect::new(0, 0, 80, 30);
        scroll_chat_thread(&mut state, &swarm, area, -8);
        assert_ne!(
            state.multipane.as_ref().unwrap().panes[0].chat_thread_scroll,
            nit_core::CONSOLE_SCROLL_BOTTOM,
            "PgUp must NEVER re-engage the follow-bottom sentinel — even when \
             max_scroll transiently dips to ≤ next"
        );
    }

    #[test]
    fn wheel_down_past_bottom_still_re_engages_sentinel() {
        // Counter-test for the delta-guard: wheel-down (delta > 0) past
        // max_scroll is the only path that should re-arm the sentinel.
        let mut state = fixture_state_no_backend();
        if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(1) {
            pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-01".into());
            pane.agent_id = "claude-haiku-4-5#mp-pane-01".into();
            pane.chat_thread_scroll = 0;
        }
        let swarm = SwarmRuntime::default();
        let area = Rect::new(0, 0, 80, 30);
        let pane1_rect = grid::pane_rect(area, 2, 1, 1);
        handle_mouse_scroll(
            &mut state,
            &swarm,
            area,
            pane1_rect.x + 5,
            pane1_rect.y + 5,
            5,
        );
        assert_eq!(
            state.multipane.as_ref().unwrap().panes[1].chat_thread_scroll,
            nit_core::CONSOLE_SCROLL_BOTTOM,
            "wheel-DOWN past the current bottom must re-engage the sentinel"
        );
    }

    // ----- BUG 4: paint_bar fills the rect with bg style -------------------

    #[test]
    fn paint_bar_fills_full_rect_with_bg_style() {
        use ratatui::backend::TestBackend;
        use ratatui::style::Color;
        use ratatui::Terminal;

        let backend = TestBackend::new(40, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let style = Style::default()
            .fg(Color::White)
            .bg(Color::Blue)
            .add_modifier(Modifier::BOLD);
        let target_rect = Rect::new(0, 0, 40, 1);
        terminal
            .draw(|frame| {
                paint_bar(frame, target_rect, "MULTIPANE".into(), style);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        for x in 0..target_rect.width {
            let cell = buffer.get(target_rect.x + x, target_rect.y);
            assert_eq!(
                cell.bg,
                Color::Blue,
                "cell at column {x} must inherit the bar's bg colour"
            );
        }
    }
}
