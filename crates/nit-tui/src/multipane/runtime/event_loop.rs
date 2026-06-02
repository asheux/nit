use std::collections::HashMap;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind};
use nit_core::AppState;
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::Style,
    widgets::{Block, BorderType, Borders, Clear},
    Terminal,
};

use super::keys::handle_key;
use super::mouse::handle_mouse;
use super::render::render_grid;
use crate::app::{clear_chat_esc_state, is_global_quit_key};
use crate::claude_runner::{ClaudeRunner, ClaudeRunnerConfig};
use crate::codex_runner::{CodexRunner, CodexRunnerConfig, CodexRuntimeMode};
use crate::multipane::dir_search_runner::{DirSearchEvent, DirSearchRunner};
use crate::multipane::dispatch::with_pane_aliased;
use crate::multipane::persistence;
use crate::shadow::ShadowRuntime;
use crate::swarm::SwarmRuntime;
use crate::theme::Theme;
use crate::vitals::VitalsState;
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
    // Idle-sleep guard mirrors the single-pane runner so multipane sessions
    // also survive the macOS inactivity timer mid-swarm. See
    // `crate::power::IdleSleepGuard`.
    let mut idle_sleep_guard = crate::power::IdleSleepGuard::default();
    clear_chat_esc_state();
    let mut clipboard: Option<arboard::Clipboard> = arboard::Clipboard::new().ok();
    // Per-pane terminal PTYs (Ctrl+\), keyed by pane index and reconciled
    // against each pane's `terminal_active` flag. nit-core owns no subprocess.
    let mut terminals: HashMap<usize, crate::pty::PtySession> = HashMap::new();
    // The one-per-process modal terminal popup (Ctrl+Shift+T), overlaid on the
    // whole grid. Persists across hide; killed at quit via `finalize_session`.
    let mut terminal_popup: Option<crate::pty::PtySession> = None;

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
    // Mirror the single-pane runner's frame-rate cap so a high-volume
    // bus burst can't repaint faster than the terminal compositor. Resolved
    // once via the same `NIT_TUI_FPS` env knob the single-pane path uses.
    let frame_interval = crate::app::frame_interval();
    let mut last_render = Instant::now()
        .checked_sub(frame_interval)
        .unwrap_or_else(Instant::now);

    loop {
        // Drain any already-buffered input BEFORE the agent-bus drain so
        // wheel / PgUp / Ctrl+Q events don't get queued behind a 100+
        // event swarm burst. Non-blocking drain (capped at 32 per pass)
        // so a runaway producer can't starve the bus. The bottom-of-loop
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
                &mut terminals,
                &terminal_popup,
                &mut clipboard,
                theme,
                area,
            )?;
            if should_quit {
                finalize_session(
                    state,
                    &workspace_root,
                    had_prior_session,
                    &mut terminals,
                    &mut terminal_popup,
                );
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
        let mut codex_batch: Vec<nit_core::AgentBusEvent> = codex.events.try_iter().collect();
        if !codex_batch.is_empty() {
            crate::app::event_coalesce::coalesce_heartbeats(&mut codex_batch);
            for event in codex_batch {
                crate::app::event_drain::drain_codex_event(
                    state,
                    &mut vitals,
                    &codex,
                    &claude,
                    &mut swarm,
                    &mut shadow,
                    None,
                    None,
                    event,
                );
            }
        }
        let mut claude_batch: Vec<nit_core::AgentBusEvent> = claude.events.try_iter().collect();
        if !claude_batch.is_empty() {
            crate::app::event_coalesce::coalesce_heartbeats(&mut claude_batch);
            for event in claude_batch {
                crate::app::event_drain::drain_claude_event(
                    state,
                    &mut vitals,
                    &codex,
                    &claude,
                    &mut swarm,
                    &mut shadow,
                    None,
                    None,
                    event,
                );
            }
        }

        // Async backend-probe events (cache-miss path of init_agents).
        // See app/runner.rs for the single-pane mirror.
        for event in nit_core::agent_bus::async_queue::drain() {
            event.apply(state);
        }
        for event in dir_search_runner.events.try_iter() {
            apply_dir_search_event(state, event);
        }

        // Reconcile the idle-sleep guard with the current in-flight count.
        // Cheap when nothing changed; covers the multipane case where
        // several panes can have parallel turns but each pane's
        // TurnStarted/Completed flows through the same `active_turns` map.
        idle_sleep_guard.sync(
            state.settings.power.prevent_idle_sleep_during_turns,
            state.agents.active_turns.len(),
        );

        // Poll background genome work the same way the single-pane
        // runner does (`app/runner.rs`). Without these, the per-pane
        // swarm runs spawn their genome gate / review worker threads,
        // the workers complete and post their result on an mpsc, and
        // nobody ever reads the channel — so the breather sticks at
        // "Verifying (genome gate) ..." forever and the verifier never
        // dispatches.
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

        let term_area = terminal_size(terminal)?;
        reconcile_pane_terminals(state, &mut terminals, term_area);
        reconcile_grid_terminal_popup(state, &mut terminal_popup, term_area);

        // Render only when the per-frame minimum interval has elapsed.
        // `frame_count` advances INSIDE the gate so the breather phase
        // tracks real frames rather than loop iterations — without this,
        // a high-event-rate burst would visibly speed up the histogram
        // glyph next to "Working …" / "Verifying …" relative to the
        // single-pane path.
        if last_render.elapsed() >= frame_interval {
            state.metrics.frame_count = state.metrics.frame_count.wrapping_add(1);

            terminal.draw(|frame| {
                let area = frame.size();
                // Rebuilt every frame: terminals register their on-screen region
                // + visible text so the mouse handler can hit-test + copy.
                state.terminal_select_regions.clear();
                let cursor = render_grid(frame, area, state, &swarm, theme);
                let term_cursor = overlay_pane_terminals(frame, area, state, &terminals, theme);
                if state.agents.artifacts_popup_open {
                    let popup_area = popup_rect_for(area, artifacts_popup::preferred_size(area));
                    artifacts_popup::render(frame, popup_area, state, &swarm, theme);
                }
                // Modal terminal popup overlays the entire grid, topmost.
                let popup_cursor = if state.terminal_popup.visible {
                    terminal_popup.as_ref().and_then(|session| {
                        let inner = crate::widgets::terminal_popup::popup_inner_rect(area);
                        let lines =
                            crate::widgets::terminal_view::visible_text_lines(inner, session);
                        let selection = state.ui_selection;
                        state
                            .terminal_select_regions
                            .push(nit_core::TerminalSelectRegion {
                                pane: nit_core::UiSelectionPane::TerminalPopup,
                                x: inner.x,
                                y: inner.y,
                                width: inner.width,
                                height: inner.height,
                                lines,
                            });
                        let cwd = state.terminal_popup.cwd.as_deref();
                        crate::widgets::terminal_popup::render(
                            frame,
                            area,
                            session,
                            cwd,
                            theme,
                            selection.as_ref(),
                        )
                    })
                } else {
                    None
                };
                if let Some((x, y)) = popup_cursor {
                    frame.set_cursor(x, y);
                } else if let Some((x, y)) = term_cursor {
                    frame.set_cursor(x, y);
                } else if let Some(c) = cursor {
                    frame.set_cursor(c.x, c.y);
                }
            })?;
            // Match the single-pane chat-input caret shape so the operator
            // sees the same thin steady bar across both modes; the visible
            // blink comes from gating `frame.set_cursor` on
            // `cursor_visible(state)` (a frame-counter pulse), exactly
            // like single-pane.
            let _ = crossterm::execute!(
                terminal.backend_mut(),
                crossterm::cursor::SetCursorStyle::SteadyBar
            );

            last_render = Instant::now();
        }

        // Adaptive idle wait: when the next frame is closer than
        // `TICK_RATE`, wake at the frame boundary instead of the full
        // 50 ms tick so a deferred render fires promptly. The 1 ms
        // floor avoids degenerate sub-millisecond polls that some
        // terminals reduce to a busy-spin.
        let wait = frame_interval
            .saturating_sub(last_render.elapsed())
            .min(TICK_RATE)
            .max(Duration::from_millis(1));
        if !event::poll(wait)? {
            continue;
        }
        let area = terminal_size(terminal)?;
        // Coalesce a burst of input (e.g. wheel scroll fires 5–20
        // events per gesture) so we render once per batch instead of
        // once per event. Bounded at 32 so a runaway producer can't
        // starve the redraw indefinitely.
        let should_quit = drain_input_burst(
            state,
            &mut vitals,
            &codex,
            &claude,
            &mut swarm,
            &mut shadow,
            &dir_search_runner,
            &mut terminals,
            &terminal_popup,
            &mut clipboard,
            theme,
            area,
        )?;
        if should_quit {
            finalize_session(
                state,
                &workspace_root,
                had_prior_session,
                &mut terminals,
                &mut terminal_popup,
            );
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

/// Scroll whichever embedded terminal the multipane pointer sits over: the
/// modal popup when it's open, otherwise the per-pane terminal under the
/// cursor. Geometry comes straight from the grid (no snapshot), so any visible
/// pane terminal scrolls, not only the focused one. Returns true when the wheel
/// was consumed (including a modal-absorbed scroll behind the popup).
fn scroll_terminal_mp(
    state: &AppState,
    terminals: &HashMap<usize, crate::pty::PtySession>,
    terminal_popup: &Option<crate::pty::PtySession>,
    area: Rect,
    mouse: &MouseEvent,
) -> bool {
    const LINES: usize = 3;
    let up = match mouse.kind {
        MouseEventKind::ScrollUp => true,
        MouseEventKind::ScrollDown => false,
        _ => return false,
    };
    let scroll = |session: &crate::pty::PtySession| {
        if up {
            session.scroll_up(LINES);
        } else {
            session.scroll_down(LINES);
        }
    };
    if state.terminal_popup.visible {
        // Modal: only the popup scrolls; panes behind it stay put.
        if let Some(session) = terminal_popup.as_ref() {
            let inner = crate::widgets::terminal_popup::popup_inner_rect(area);
            if super::mouse::point_in_rect(mouse.column, mouse.row, inner) {
                scroll(session);
            }
        }
        return true;
    }
    let Some(mp) = state.multipane.as_ref() else {
        return false;
    };
    // `handle_mouse` strips the top status strip + bottom hint row before
    // hit-testing panes; mirror that so the pointer maps to the right pane.
    let grid_area = if area.height >= 4 {
        Rect::new(area.x, area.y + 1, area.width, area.height - 2)
    } else {
        area
    };
    let Some(pane_idx) = crate::multipane::grid::pane_at_point(
        grid_area,
        mp.grid_cols,
        mp.grid_rows,
        mouse.column,
        mouse.row,
    ) else {
        return false;
    };
    if !mp.panes.get(pane_idx).is_some_and(|p| p.terminal_active) {
        return false;
    }
    let Some(session) = terminals.get(&pane_idx) else {
        return false;
    };
    scroll(session);
    true
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
    terminals: &mut HashMap<usize, crate::pty::PtySession>,
    terminal_popup: &Option<crate::pty::PtySession>,
    clipboard: &mut Option<arboard::Clipboard>,
    theme: &Theme,
    area: Rect,
) -> io::Result<bool> {
    // Drag events fire ~30/burst on fast mouse movement; each one
    // calls build_lines via the popup / chat-thread mappers (markdown
    // render + syntax highlighting). Coalesce by deferring drag events
    // into `pending_drag` and only handling the LAST one — flushed
    // when a non-drag event arrives or at end-of-drain. Auto-scroll
    // overshoot already scales with mouse distance past the edge, so
    // dropping intermediate drags doesn't hurt scroll throughput; it
    // just stops the build_lines burst that was causing the visible
    // lag.
    let mut pending_drag: Option<MouseEvent> = None;
    for _ in 0..32 {
        match event::read()? {
            // Accept both Press and Repeat so held keys (Backspace,
            // Delete, arrow nav, character repeats) auto-fire — single
            // pane already does this in `app/runner.rs` and the UX
            // mismatch was breaking long edits in multipane.
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if let Some(drag) = pending_drag.take() {
                    handle_mouse(state, swarm, theme, clipboard, area, drag);
                }
                // The modal popup is top-level: while visible it swallows every
                // key, forwarding to its shell except the two close intercepts.
                if state.terminal_popup.visible {
                    forward_key_to_terminal_popup(state, terminal_popup, key);
                } else if !forward_key_to_focused_terminal(state, terminals, key)
                    && handle_key(
                        state, vitals, codex, claude, swarm, shadow, dir_runner, key, clipboard,
                        area,
                    )
                {
                    return Ok(true);
                }
            }
            // Bracketed paste arrives as a single text blob (not a
            // sequence of Char key events), so without this branch
            // Cmd-V / right-click-paste / iTerm paste in multipane is
            // silently dropped.
            Event::Paste(text) => {
                if let Some(drag) = pending_drag.take() {
                    handle_mouse(state, swarm, theme, clipboard, area, drag);
                }
                handle_paste(state, &text);
            }
            Event::Mouse(mouse) => {
                // Wheel over any terminal grid scrolls that session instead of
                // the chat thread underneath (modal popup wins when open).
                if scroll_terminal_mp(state, terminals, terminal_popup, area, &mouse) {
                    if let Some(drag) = pending_drag.take() {
                        handle_mouse(state, swarm, theme, clipboard, area, drag);
                    }
                } else if state.terminal_popup.visible {
                    // Modal popup: route selection (down/drag/up) through the
                    // mouse handler; an outside-left-down closes it. Other
                    // events behind the popup are absorbed.
                    let popup_area = crate::widgets::terminal_popup::popup_rect(area);
                    match mouse.kind {
                        MouseEventKind::Down(crossterm::event::MouseButton::Left)
                            if !super::mouse::point_in_rect(
                                mouse.column,
                                mouse.row,
                                popup_area,
                            ) =>
                        {
                            if let Some(drag) = pending_drag.take() {
                                handle_mouse(state, swarm, theme, clipboard, area, drag);
                            }
                            state.terminal_popup.toggle_requested = true;
                        }
                        MouseEventKind::Drag(_) => {
                            pending_drag = Some(mouse);
                        }
                        _ => {
                            if let Some(drag) = pending_drag.take() {
                                handle_mouse(state, swarm, theme, clipboard, area, drag);
                            }
                            handle_mouse(state, swarm, theme, clipboard, area, mouse);
                        }
                    }
                } else if matches!(mouse.kind, MouseEventKind::Drag(_)) {
                    pending_drag = Some(mouse);
                } else {
                    if let Some(drag) = pending_drag.take() {
                        handle_mouse(state, swarm, theme, clipboard, area, drag);
                    }
                    handle_mouse(state, swarm, theme, clipboard, area, mouse);
                }
            }
            _ => {}
        }
        if !event::poll(Duration::from_millis(0))? {
            break;
        }
    }
    if let Some(drag) = pending_drag.take() {
        handle_mouse(state, swarm, theme, clipboard, area, drag);
    }
    Ok(false)
}

/// Routes a bracketed-paste blob to whichever input is currently
/// receiving keystrokes: the artifacts popup chat input when it's
/// open, otherwise the focused pane's chat input.
fn handle_paste(state: &mut AppState, text: &str) {
    if text.is_empty() {
        return;
    }
    if state.agents.artifacts_popup_open {
        let _ = crate::app::insert_popup_chat_text(state, text);
        return;
    }
    let pane_idx = super::keys::focused_pane_idx(state);
    with_pane_aliased(state, pane_idx, |state| {
        let _ = crate::app::insert_chat_input_text(state, text);
    });
}

// Discard the session file if nothing was run yet and no prior file
// existed; otherwise persist so the operator can resume.
fn finalize_session(
    state: &AppState,
    workspace_root: &Path,
    had_prior: bool,
    terminals: &mut HashMap<usize, crate::pty::PtySession>,
    terminal_popup: &mut Option<crate::pty::PtySession>,
) {
    for (_, mut session) in terminals.drain() {
        session.shutdown();
    }
    if let Some(mut session) = terminal_popup.take() {
        session.shutdown();
    }
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
/// (which is `pub(super)` on the single-pane app side). Local copy keeps
/// multipane independent of the single-pane app module's private
/// layout helpers.
pub(super) fn popup_rect_for(screen: Rect, desired: (u16, u16)) -> Rect {
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

pub(in crate::multipane) fn capture_pane_mission_ids(state: &mut AppState) {
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

pub(super) fn apply_dir_search_event(state: &mut AppState, event: DirSearchEvent) {
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

/// Forward a keystroke to the focused pane's live terminal. The `Ctrl+\` toggle
/// and the global quit fall through to `handle_key` so the operator can always
/// escape. Returns whether the terminal consumed the key.
fn forward_key_to_focused_terminal(
    state: &AppState,
    terminals: &HashMap<usize, crate::pty::PtySession>,
    key: KeyEvent,
) -> bool {
    let idx = super::keys::focused_pane_idx(state);
    let active = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(idx))
        .map(|pane| pane.terminal_active)
        .unwrap_or(false);
    if !active || crate::pty::is_terminal_toggle_key(&key) || is_global_quit_key(&key) {
        return false;
    }
    if let Some(session) = terminals.get(&idx) {
        if let Some(bytes) = crate::pty::encode_key(key) {
            let _ = session.write_input(&bytes);
        }
    }
    true
}

/// Route a key to the focused modal popup: the two intercepts request a close
/// (serviced by `reconcile_grid_terminal_popup`), everything else forwards to
/// the shell.
fn forward_key_to_terminal_popup(
    state: &mut AppState,
    popup: &Option<crate::pty::PtySession>,
    key: KeyEvent,
) {
    match crate::app::popup_keys::terminal_popup_key(&key) {
        crate::app::popup_keys::TerminalPopupKey::Close => {
            state.terminal_popup.toggle_requested = true;
        }
        crate::app::popup_keys::TerminalPopupKey::Forward(bytes) => {
            if let Some(session) = popup.as_ref() {
                let _ = session.write_input(&bytes);
            }
        }
        crate::app::popup_keys::TerminalPopupKey::ForwardAndClose(bytes) => {
            if let Some(session) = popup.as_ref() {
                let _ = session.write_input(&bytes);
            }
            state.terminal_popup.toggle_requested = true;
        }
        crate::app::popup_keys::TerminalPopupKey::Ignore => {}
    }
}

/// cwd for a popup opened in multipane: the focused pane's directory, falling
/// back to the workspace root when that pane has none.
fn focused_pane_cwd(state: &AppState) -> PathBuf {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .map(|pane| pane.cwd.clone())
        .filter(|cwd| !cwd.as_os_str().is_empty())
        .unwrap_or_else(|| state.workspace_root.clone())
}

/// Service a pending popup toggle over the grid: pin the focused pane's cwd
/// (re-pinned only after the prior shell exits), flip visibility, and spawn a
/// fresh shell when opening without a live one. Close only HIDES.
fn reconcile_grid_terminal_popup(
    state: &mut AppState,
    popup: &mut Option<crate::pty::PtySession>,
    area: Rect,
) {
    if !std::mem::take(&mut state.terminal_popup.toggle_requested) {
        return;
    }
    let exited = popup.as_ref().is_some_and(|s| s.has_exited());
    let cwd = focused_pane_cwd(state);
    state.terminal_popup.apply_toggle(&cwd, exited);
    let needs_spawn = popup.is_none() || popup.as_ref().is_some_and(|s| s.has_exited());
    if state.terminal_popup.visible && needs_spawn {
        if let Some(mut dead) = popup.take() {
            dead.shutdown();
        }
        let dir = state.terminal_popup.cwd.clone().unwrap_or(cwd);
        let popup_area = crate::widgets::terminal_popup::popup_rect(area);
        let size = crate::pty::PtySize {
            rows: popup_area.height.saturating_sub(2).max(1),
            cols: popup_area.width.saturating_sub(2).max(1),
        };
        match crate::pty::PtySession::spawn(&dir, size) {
            Ok(session) => *popup = Some(session),
            Err(err) => {
                state.terminal_popup.visible = false;
                state.status = Some(format!("Terminal popup: {err}"));
            }
        }
    }
}

/// Spawn/kill per-pane terminals to match each pane's `terminal_active` flag,
/// resync live winsizes, and revert a pane to chat when its shell exits.
fn reconcile_pane_terminals(
    state: &mut AppState,
    terminals: &mut HashMap<usize, crate::pty::PtySession>,
    area: Rect,
) {
    let snapshot: Vec<(usize, bool, PathBuf)> = match state.multipane.as_ref() {
        Some(mp) => mp
            .panes
            .iter()
            .enumerate()
            .map(|(idx, pane)| (idx, pane.terminal_active, pane.cwd.clone()))
            .collect(),
        None => return,
    };
    let (cols, rows) = state
        .multipane
        .as_ref()
        .map(|mp| (mp.grid_cols, mp.grid_rows))
        .unwrap_or((0, 0));
    terminals.retain(|idx, _| *idx < snapshot.len());
    for (idx, active, cwd) in snapshot {
        // Reap a session that exited (operator ran `exit`, or it
        // crashed). If the pane was visible, also revert it to chat
        // so the operator doesn't stare at a dead shell. If it was
        // parked we just drop the dead session silently — flipping
        // back to TERM will spawn fresh.
        if terminals.get(&idx).is_some_and(|s| s.has_exited()) {
            if let Some(mut session) = terminals.remove(&idx) {
                session.shutdown();
            }
            if active {
                set_pane_terminal_active(state, idx, false);
            }
            continue;
        }
        let size = pane_terminal_inner(area, cols, rows, idx)
            .map(|inner| crate::pty::PtySize {
                rows: inner.height,
                cols: inner.width,
            })
            .unwrap_or(crate::pty::PtySize { rows: 24, cols: 80 });
        match (active, terminals.contains_key(&idx)) {
            (true, false) => match crate::pty::PtySession::spawn(&cwd, size) {
                Ok(session) => {
                    terminals.insert(idx, session);
                }
                Err(_) => set_pane_terminal_active(state, idx, false),
            },
            (true, true) => {
                if let Some(session) = terminals.get(&idx) {
                    let _ = session.resize(size);
                }
            }
            // PARKED: keep the session alive across NIT → TERM → NIT
            // flips so history, running commands and the shell's
            // $PWD survive. The only teardown is reaping above (on
            // natural exit) and the runner's drop path at quit, which
            // takes every session.
            (false, true) => {}
            (false, false) => {}
        }
    }
}

fn set_pane_terminal_active(state: &mut AppState, idx: usize, value: bool) {
    if let Some(mp) = state.multipane.as_mut() {
        if let Some(pane) = mp.panes.get_mut(idx) {
            pane.terminal_active = value;
        }
    }
}

/// Inner (chrome-stripped) rect for a pane's terminal grid. Mirrors
/// `render_grid`'s reserved top/bottom strips plus an all-borders block, so the
/// PTY winsize matches what `overlay_pane_terminals` paints.
fn pane_terminal_inner(area: Rect, cols: usize, rows: usize, idx: usize) -> Option<Rect> {
    let chrome = area.height >= 4;
    let grid_area = if chrome {
        Rect::new(area.x, area.y + 1, area.width, area.height - 2)
    } else {
        area
    };
    let rect = crate::multipane::grid::pane_rect(grid_area, cols, rows, idx);
    if rect.width < 2 || rect.height < 2 {
        return None;
    }
    Some(Rect::new(
        rect.x + 1,
        rect.y + 1,
        rect.width - 2,
        rect.height - 2,
    ))
}

/// Paint each active pane's terminal over the chat render it replaces. Returns
/// the focused pane's cursor cell — rendered last so it wins the frame slot.
fn overlay_pane_terminals(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &mut AppState,
    terminals: &HashMap<usize, crate::pty::PtySession>,
    theme: &Theme,
) -> Option<(u16, u16)> {
    if terminals.is_empty() {
        return None;
    }
    let (focused, cols, rows) = state
        .multipane
        .as_ref()
        .map(|mp| (mp.focused, mp.grid_cols, mp.grid_rows))?;
    let chrome = area.height >= 4;
    let grid_area = if chrome {
        Rect::new(area.x, area.y + 1, area.width, area.height - 2)
    } else {
        area
    };
    // Only paint sessions whose pane is currently showing the TERM
    // tab. `reconcile_pane_terminals` keeps parked sessions alive in
    // the HashMap across NIT ↔ TERM flips so history survives, but
    // we must NOT render them over the chat when the operator
    // switched back to NIT — without this filter the chat is
    // invisible behind the parked terminal block.
    let mp_panes = state.multipane.as_ref().map(|mp| &mp.panes);
    let mut order: Vec<usize> = terminals.keys().copied().collect();
    order.sort_unstable_by_key(|idx| *idx == focused);
    let mut focused_cursor = None;
    for idx in order {
        let active = mp_panes
            .and_then(|panes| panes.get(idx))
            .is_some_and(|pane| pane.terminal_active);
        if !active {
            continue;
        }
        let Some(session) = terminals.get(&idx) else {
            continue;
        };
        let rect = crate::multipane::grid::pane_rect(grid_area, cols, rows, idx);
        if rect.width < 2 || rect.height < 2 {
            continue;
        }
        let is_focused = idx == focused;
        let cwd_text = state
            .multipane
            .as_ref()
            .and_then(|mp| mp.panes.get(idx))
            .map(|pane| crate::multipane::runtime::render::pane_path_label(state, &pane.cwd))
            .unwrap_or_default();
        let title_line = crate::multipane::runtime::render::pane_tabs_line(
            idx,
            crate::multipane::runtime::render::PaneTab::Terminal,
            &cwd_text,
            is_focused,
            theme,
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(if is_focused {
                BorderType::Thick
            } else {
                BorderType::Plain
            })
            .border_style(Style::default().fg(if is_focused {
                theme.border_focused
            } else {
                theme.border
            }))
            .title(title_line)
            .style(Style::default().bg(theme.background));
        let inner = block.inner(rect);
        frame.render_widget(Clear, rect);
        frame.render_widget(block, rect);
        // Terminal selection is scoped to the focused pane (clicking a pane
        // focuses it), so only the focused terminal registers a selectable
        // region and receives the selection overlay — the shared `Terminal`
        // pane id can't disambiguate multiple simultaneous grids otherwise.
        let selection = if is_focused { state.ui_selection } else { None };
        if is_focused {
            let lines = crate::widgets::terminal_view::visible_text_lines(inner, session);
            state
                .terminal_select_regions
                .push(nit_core::TerminalSelectRegion {
                    pane: nit_core::UiSelectionPane::Terminal,
                    x: inner.x,
                    y: inner.y,
                    width: inner.width,
                    height: inner.height,
                    lines,
                });
        }
        crate::widgets::terminal_view::render_screen(
            frame,
            inner,
            session,
            theme,
            selection.as_ref(),
            nit_core::UiSelectionPane::Terminal,
        );
        if is_focused {
            focused_cursor = crate::widgets::terminal_view::cursor_position(inner, session);
        }
    }
    focused_cursor
}
