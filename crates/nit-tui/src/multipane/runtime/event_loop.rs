use std::io::{self, Stdout};
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind, MouseEvent, MouseEventKind};
use nit_core::AppState;
use ratatui::{backend::CrosstermBackend, layout::Rect, Terminal};

use super::keys::handle_key;
use super::mouse::handle_mouse;
use super::render::render_grid;
use crate::app::clear_chat_esc_state;
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

        // Animate the breather. The single-pane runner ticks
        // `frame_count` in `app/draw.rs:408`; multipane has its own draw
        // path so we have to do it explicitly. Without this, the
        // histogram glyph next to "Working ..." / "Verifying ..." stays
        // frozen on a single frame regardless of how long an agent
        // runs.
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
        // sees the same thin steady bar across both modes; without
        // this, multipane inherits whatever the terminal's default is
        // (usually a wide block) and the caret looks fat/inconsistent.
        // The bar is "steady" — the visible blink comes from gating
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
            Event::Paste(text) => {
                if let Some(drag) = pending_drag.take() {
                    handle_mouse(state, swarm, theme, clipboard, area, drag);
                }
                handle_paste(state, &text);
            }
            Event::Mouse(mouse) => {
                if matches!(mouse.kind, MouseEventKind::Drag(_)) {
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
