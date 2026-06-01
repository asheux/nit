#![allow(clippy::too_many_arguments)]
use std::io::{self, Stdout};
use std::time::Instant;

use crossterm::{cursor::SetCursorStyle, execute};
use nit_core::{AgentOpsTab, AppKind, AppState, Mode, PaneId, Prompt, SearchMode};
use ratatui::{
    backend::CrosstermBackend,
    style::Style,
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Terminal,
};

use crate::{
    fuzzy_preview_runner::PreviewModel,
    games_petri_dish::GamesPetriDishRuntime,
    layout,
    petri_dish::PetriDishRuntime,
    seed_runtime::SeedRuntime,
    swarm::SwarmRuntime,
    syntax::SyntaxRuntime,
    system_stats::SystemStats,
    theme::Theme,
    vitals::LabVitalsSnapshot,
    widgets::{
        agent_console_view, agent_ops_view, artifacts_history_popup, artifacts_popup, bottom_bar,
        definition_popup, editor_view, file_tree_view, fuzzy_search_popup, games_analysis_popup,
        games_ca_sim_popup, games_match_history_popup, games_replay_popup, games_run_browser_popup,
        games_strategy_popup, games_tm_sim_popup, games_visualizer_view, gate_monitor_view,
        help_overlay, rule_picker, substrate_overlay, terminal_view, top_bar, visualizer_view,
    },
};

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut AppState,
    swarm: &SwarmRuntime,
    workspace_scan: &crate::workspace_scan::WorkspaceScanRuntime,
    theme: &Theme,
    syntax: &mut SyntaxRuntime,
    system_stats: &SystemStats,
    seed_runtime: &mut Option<SeedRuntime>,
    gol_petri: &mut Option<PetriDishRuntime>,
    games_petri: &mut Option<GamesPetriDishRuntime>,
    fuzzy_preview: Option<&PreviewModel>,
    fuzzy_preview_scroll_delta: &mut i32,
    terminal_pane: Option<&crate::pty::PtySession>,
    terminal_popup: Option<&crate::pty::PtySession>,
    terminal_popup_live_cwd: Option<&std::path::Path>,
    vitals: &LabVitalsSnapshot,
) -> io::Result<()> {
    let start = Instant::now();
    terminal.draw(|f| {
        let screen = f.size();
        let layout = layout::split(screen);

        // Update viewports (account for gutters)
        let editor_total = state.editor_buffer().lines_len().max(1);
        let editor_line_width = editor_total.to_string().len().max(3) as u16;
        let editor_gutter = editor_line_width + 4;
        let editor_text_width = layout
            .editor
            .width
            .saturating_sub(2)
            .saturating_sub(editor_gutter);
        let editor_height = layout.editor.height.saturating_sub(2) as usize;
        let editor_width = editor_text_width as usize;
        {
            let buf = state.editor_buffer_mut();
            let resized =
                buf.viewport.height != editor_height || buf.viewport.width != editor_width;
            buf.set_viewport_size(editor_height, editor_width);
            if resized {
                buf.ensure_visible();
            }
        }
        let editor_id = state.active_editor_buffer_id;
        let notes_id = state.notes_buffer_id;
        top_bar::render(f, layout.top, state, theme, vitals);
        let editor_cursor = if state.file_tree.open {
            adjust_file_tree_scroll(state, layout.editor);
            file_tree_view::render(f, layout.editor, state, theme);
            None
        } else {
            state.editor_buffer_mut().compute_diff_if_needed();
            let editor_render = syntax.render_snapshot_for(editor_id, state.editor_buffer());
            // While the `/` prompt is open the live input takes
            // priority — every keystroke restyles matches in real time;
            // once Enter commits the term we fall back to the persistent
            // `editor_search` highlight that `n` / `N` step through.
            let live_prompt = state
                .search_prompt
                .as_ref()
                .map(|p| p.input.as_str())
                .filter(|s| !s.is_empty());
            let search = if let Some(term) = live_prompt {
                Some(editor_view::SearchHighlight {
                    term,
                    whole_word: false,
                    case_insensitive: nit_core::state::smart_case_insensitive(term),
                    live: true,
                })
            } else {
                state
                    .editor_search
                    .term
                    .as_deref()
                    .map(|t| editor_view::SearchHighlight {
                        term: t,
                        whole_word: state.editor_search.whole_word,
                        case_insensitive: nit_core::state::smart_case_insensitive(t),
                        live: false,
                    })
            };
            editor_view::render_editor_with_search(
                f,
                layout.editor,
                state.editor_buffer(),
                editor_render.snapshot,
                editor_render.line_map,
                state.focus,
                state.mode,
                theme,
                state.settings.editor.tab_width as usize,
                search,
            )
        };
        let mut terminal_cursor: Option<(u16, u16)> = None;
        let notes_cursor = match (state.terminal_pane_active, terminal_pane) {
            (true, Some(session)) => {
                let focused = state.focus == PaneId::Notes;
                // Render the same AGENT CHAT / TERMINAL tab line the
                // chat console uses so the operator can click AGENT
                // CHAT to switch back. `chat_tab_at_column` in
                // app/mouse.rs hit-tests the same columns.
                let title_line = agent_console_view::chat_pane_title_line(
                    agent_console_view::ChatPaneTab::Terminal,
                    focused,
                    theme,
                );
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Thick)
                    .title(title_line)
                    .border_style(Style::default().fg(theme.border_focused))
                    .style(Style::default().bg(theme.background));
                let inner = block.inner(layout.notes);
                f.render_widget(block, layout.notes);
                terminal_view::render_screen(f, inner, session, theme);
                terminal_cursor = terminal_view::cursor_position(inner, session);
                None
            }
            _ => agent_console_view::render(f, layout.notes, state, swarm, theme),
        };
        {
            let text_area = job_output_text_area(layout.job);
            state.agents.ops_viewport_width = text_area.width.max(1) as usize;
            state.agents.ops_viewport_height = text_area.height.max(1) as usize;
        }
        let job_cursor = if state.agents.dock_tab == AgentOpsTab::Scratchpad {
            let focused = state.focus == PaneId::JobOutput;
            let border_style = if focused {
                Style::default().fg(theme.border_focused)
            } else {
                Style::default().fg(theme.border)
            };
            let border_type = if focused {
                BorderType::Thick
            } else {
                BorderType::Plain
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .title("AGENT OPS")
                .border_style(border_style)
                .border_type(border_type)
                .style(Style::default().bg(theme.background));
            f.render_widget(block.clone(), layout.job);
            let outer_inner = block.inner(layout.job);
            if outer_inner.width >= 4 && outer_inner.height >= 3 {
                let outer_chunks = ratatui::layout::Layout::default()
                    .direction(ratatui::layout::Direction::Vertical)
                    .constraints([
                        ratatui::layout::Constraint::Length(1),
                        ratatui::layout::Constraint::Min(1),
                    ])
                    .split(outer_inner);
                agent_ops_view::render_tab_bar(f, outer_chunks[0], state, theme);
                let scratchpad_area = outer_chunks[1];

                let notes_total = state.notes_buffer().lines_len().max(1);
                let notes_line_width = notes_total.to_string().len().max(3) as u16;
                let notes_gutter = notes_line_width + 4;
                let notes_text_width = scratchpad_area
                    .width
                    .saturating_sub(2)
                    .saturating_sub(notes_gutter);
                let notes_height = scratchpad_area.height.saturating_sub(2) as usize;
                let notes_width = notes_text_width as usize;
                {
                    let buf = state.notes_buffer_mut();
                    let resized =
                        buf.viewport.height != notes_height || buf.viewport.width != notes_width;
                    buf.set_viewport_size(notes_height, notes_width);
                    if resized {
                        buf.ensure_visible();
                    }
                }
                let notes_render = syntax.render_snapshot_for(notes_id, state.notes_buffer());
                editor_view::render_buffer(
                    f,
                    scratchpad_area,
                    state.notes_buffer(),
                    notes_render.snapshot,
                    notes_render.line_map,
                    PaneId::JobOutput,
                    state.focus,
                    "SCRATCHPAD",
                    theme,
                    state.settings.editor.tab_width as usize,
                    true,
                    state.mode,
                    None,
                )
            } else {
                None
            }
        } else {
            agent_ops_view::render(f, layout.job, state, swarm, theme);
            None
        };
        match state.app_kind {
            AppKind::Gol => {
                let viz_inner_width = layout.visualizer.width.saturating_sub(2) as usize;
                let viz_inner_height = layout.visualizer.height.saturating_sub(2) as usize;
                let viz_grid_rows = viz_inner_height.saturating_sub(1);
                let (grid_w, grid_h) = crate::seed_render::grid_size_for_mode(
                    viz_inner_width,
                    viz_grid_rows,
                    state.visualizer.seed_plate_mode,
                );
                if let Some(seed_runtime) = seed_runtime.as_mut() {
                    seed_runtime.ensure_size(grid_w, grid_h, state);
                    visualizer_view::render(f, layout.visualizer, state, theme, seed_runtime);
                }
            }
            AppKind::Games => {
                games_visualizer_view::render(
                    f,
                    layout.visualizer,
                    state,
                    theme,
                    current_games_config_result(state),
                    state.games.config_preview_pending,
                );
            }
        }
        let syntax_status = syntax.status_label_for(editor_id, state.editor_buffer().version());
        let syntax_debug = {
            let latest = syntax.latest_snapshot_for(editor_id);
            nit_core::SyntaxDebugInfo {
                buffer_version: state.editor_buffer().version(),
                snapshot_version: latest.map(|s| s.version),
                engine_state: syntax.engine_state_label(editor_id),
                last_job_ms: latest.map(|s| s.duration_ms),
            }
        };
        state.syntax_status = syntax_status.clone();
        state.syntax_debug = Some(syntax_debug.clone());
        gate_monitor_view::render(f, layout.gate, state, workspace_scan, theme);
        bottom_bar::render(f, layout.bottom, state, theme, system_stats);

        match state.app_kind {
            AppKind::Gol => {
                if let (Some(petri), Some(seed_runtime)) =
                    (gol_petri.as_mut(), seed_runtime.as_mut())
                {
                    petri.handle_pending_requests(state, seed_runtime, screen);
                    petri.render(f, screen, state, theme);
                }
            }
            AppKind::Games => {
                if let Some(petri) = games_petri.as_mut() {
                    petri.handle_pending_requests(state);
                    petri.render(f, screen, state, theme);
                }
            }
        }
        if state.app_kind == AppKind::Games && state.games.analysis.open {
            let area = dynamic_popup_rect(screen, games_analysis_popup::preferred_size(screen));
            games_analysis_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.run_browser.open {
            let area = dynamic_popup_rect(screen, games_run_browser_popup::preferred_size(screen));
            games_run_browser_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.replay.open {
            let area = dynamic_popup_rect(screen, games_replay_popup::preferred_size(screen));
            games_replay_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.strategy_inspect.open {
            let area = dynamic_popup_rect(screen, games_strategy_popup::preferred_size(screen));
            games_strategy_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.tm_sim.open {
            let area = dynamic_popup_rect(screen, games_tm_sim_popup::preferred_size(screen));
            games_tm_sim_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.ca_sim.open {
            let area = dynamic_popup_rect(screen, games_ca_sim_popup::preferred_size(screen));
            games_ca_sim_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.match_history.open {
            let area =
                dynamic_popup_rect(screen, games_match_history_popup::preferred_size(screen));
            games_match_history_popup::render(f, area, state, theme);
        }
        if state.rule_picker.open {
            rule_picker::render(f, screen, state, theme);
        }
        if state.agents.global_archive_open {
            let area = dynamic_popup_rect(screen, artifacts_history_popup::preferred_size(screen));
            artifacts_history_popup::render(f, area, state, theme);
        }
        let artifacts_popup_cursor = if state.agents.artifacts_popup_open {
            let area = dynamic_popup_rect(screen, artifacts_popup::preferred_size(screen));
            artifacts_popup::render(f, area, state, swarm, theme)
        } else {
            None
        };
        if state.show_help {
            let area = dynamic_popup_rect(screen, help_overlay::preferred_size(screen));
            help_overlay::render(f, area, state, theme);
        }
        if state.definition_popup.is_some() {
            // Same-file v1: reuse the editor's live snapshot so the snippet
            // matches the buffer's own colors with no second parse.
            let area = dynamic_popup_rect(screen, definition_popup::preferred_size(screen));
            let render = syntax.render_snapshot_for(editor_id, state.editor_buffer());
            if let Some(view) = state.definition_popup.as_ref() {
                definition_popup::render(f, area, view, render.snapshot, render.line_map, theme);
            }
        }
        if state.show_substrate_overlay {
            let area = substrate_overlay::preferred_size(screen, state.substrate_overlay_tab);
            substrate_overlay::render(f, area, state, theme);
        }
        if state.fuzzy_search.open {
            let area = dynamic_popup_rect(screen, fuzzy_popup_size(screen, state));
            fuzzy_search_popup::render(
                f,
                area,
                state,
                theme,
                fuzzy_preview,
                fuzzy_preview_scroll_delta,
            );
        }
        if let Some(Prompt::ConfirmQuit) = state.prompt {
            let message = "Quit without saving? (Y/N)";
            let area = dynamic_popup_rect(screen, prompt_size(message));
            render_prompt(f, area, theme, message);
        }
        if let Some(Prompt::ConfirmCloseBuffer) = state.prompt {
            let message = "Close buffer without saving? (Y/N)";
            let area = dynamic_popup_rect(screen, prompt_size(message));
            render_prompt(f, area, theme, message);
        }
        let mut command_cursor = None;
        if let Some(cmd) = state.command_line.as_ref() {
            let message = format!(":{}", cmd.input);
            let area = dynamic_popup_rect(screen, prompt_size(&message));
            render_command_prompt(f, area, theme, &message);
            command_cursor = command_prompt_cursor(area, &cmd.input, cmd.cursor);
        }
        if let Some(prompt) = state.search_prompt.as_ref() {
            let message = format!("/{}", prompt.input);
            let area = dynamic_popup_rect(screen, prompt_size(&message));
            render_search_prompt(f, area, theme, &message);
            command_cursor = command_prompt_cursor(area, &prompt.input, prompt.cursor);
        }
        let fuzzy_cursor = if state.fuzzy_search.open {
            let area = dynamic_popup_rect(screen, fuzzy_popup_size(screen, state));
            fuzzy_search_cursor(area, state)
        } else {
            None
        };

        // Modal terminal popup: drawn last so it dims and overlays every other
        // pane/overlay; its shell cursor takes precedence below. The live cwd
        // (polled via ShellCwdProbe in the runner) wins over the cwd pinned at
        // spawn time so `cd` inside the popup updates the title.
        let terminal_popup_cursor = if state.terminal_popup.visible {
            terminal_popup.and_then(|session| {
                let title_cwd = terminal_popup_live_cwd.or(state.terminal_popup.cwd.as_deref());
                crate::widgets::terminal_popup::render(f, screen, session, title_cwd, theme)
            })
        } else {
            None
        };

        // Cursor: only set it when we actually want a visible caret. If we don't set a cursor,
        // ratatui will hide it; calling `set_cursor(0, 0)` makes it look like the cursor is
        // jumping to the top-left corner.
        let petri_visible = match state.app_kind {
            AppKind::Gol => gol_petri.as_ref().map(|p| p.is_visible()).unwrap_or(false),
            AppKind::Games => games_petri
                .as_ref()
                .map(|p| p.is_visible())
                .unwrap_or(false),
        };
        let cursor = if let Some((x, y)) = terminal_popup_cursor {
            Some((x, y))
        } else if let Some((x, y)) = command_cursor {
            Some((x, y))
        } else if let Some((x, y)) = fuzzy_cursor {
            Some((x, y))
        } else if let Some((x, y)) = artifacts_popup_cursor {
            Some((x, y))
        } else if petri_visible || (state.file_tree.open && state.focus == PaneId::Editor) {
            None
        } else if state.focus == PaneId::Editor {
            editor_cursor.map(|pos| (pos.x, pos.y))
        } else if state.focus == PaneId::JobOutput
            && state.agents.dock_tab == AgentOpsTab::Scratchpad
        {
            job_cursor.map(|pos| (pos.x, pos.y))
        } else if state.terminal_pane_active {
            terminal_cursor
        } else {
            notes_cursor.map(|pos| (pos.x, pos.y))
        };
        if let Some((x, y)) = cursor {
            f.set_cursor(x, y);
        }
    })?;
    // Apply cursor style after draw so ratatui's cursor-show/hide logic has run,
    // but inside the same flush window (no visible frame gap).
    let bar_caret = state.agents.artifacts_popup_open
        || state.focus == PaneId::Notes
        || matches!(state.mode, Mode::Insert);
    let cursor_style = if bar_caret {
        SetCursorStyle::SteadyBar
    } else {
        SetCursorStyle::SteadyBlock
    };
    execute!(terminal.backend_mut(), cursor_style)?;
    state.metrics.last_render_ms = start.elapsed().as_millis();
    state.metrics.frame_count += 1;
    Ok(())
}

pub(super) fn prompt_size(message: &str) -> (u16, u16) {
    let width = message.chars().count().max(12) as u16 + 4;
    let height = 3;
    (width, height)
}

pub(super) fn command_prompt_cursor(
    area: ratatui::layout::Rect,
    input: &str,
    cursor: usize,
) -> Option<(u16, u16)> {
    if area.width < 3 || area.height < 3 {
        return None;
    }
    let inner_width = area.width.saturating_sub(2);
    if inner_width == 0 {
        return None;
    }
    let inner_x = area.x.saturating_add(1);
    let inner_y = area.y.saturating_add(1);
    let prefix_width = unicode_width::UnicodeWidthStr::width(":");
    let cursor_text: String = input.chars().take(cursor).collect();
    let cursor_width = unicode_width::UnicodeWidthStr::width(cursor_text.as_str());
    let mut col = inner_x.saturating_add((prefix_width + cursor_width) as u16);
    let max_col = inner_x.saturating_add(inner_width.saturating_sub(1));
    if col > max_col {
        col = max_col;
    }
    Some((col, inner_y))
}

pub(super) fn fuzzy_search_cursor(
    area: ratatui::layout::Rect,
    state: &AppState,
) -> Option<(u16, u16)> {
    if area.width < 3 || area.height < 3 {
        return None;
    }
    let inner_width = area.width.saturating_sub(2);
    if inner_width == 0 {
        return None;
    }
    let inner_x = area.x.saturating_add(1);
    let inner_y = area.y.saturating_add(1);
    let mode = match state.fuzzy_search.mode {
        SearchMode::Files => "[FILES]",
        SearchMode::Content => "[CONTENT]",
    };
    let prefix_width =
        unicode_width::UnicodeWidthStr::width(mode) + unicode_width::UnicodeWidthStr::width("  > ");
    let query_width = unicode_width::UnicodeWidthStr::width(state.fuzzy_search.query.as_str());
    let mut col = inner_x.saturating_add((prefix_width + query_width) as u16);
    let max_col = inner_x.saturating_add(inner_width.saturating_sub(1));
    if col > max_col {
        col = max_col;
    }
    Some((col, inner_y))
}

fn render_titled_prompt(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    title: &str,
    message: &str,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title(title)
        .style(Style::default().bg(theme.selection_bg));
    let paragraph = Paragraph::new(message)
        .style(Style::default().bg(theme.selection_bg).fg(theme.foreground))
        .block(block);
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

pub(super) fn render_prompt(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    message: &str,
) {
    render_titled_prompt(frame, area, theme, "CONFIRM", message);
}

pub(super) fn render_command_prompt(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    message: &str,
) {
    render_titled_prompt(frame, area, theme, "COMMAND", message);
}

pub(super) fn render_search_prompt(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    message: &str,
) {
    render_titled_prompt(frame, area, theme, "SEARCH", message);
}
