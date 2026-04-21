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
            let search = state
                .editor_search
                .term
                .as_deref()
                .map(|t| (t, state.editor_search.whole_word));
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
        let notes_cursor = agent_console_view::render(f, layout.notes, state, swarm, theme);
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
        let cursor = if let Some((x, y)) = command_cursor {
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
        } else {
            notes_cursor.map(|pos| (pos.x, pos.y))
        };
        if let Some((x, y)) = cursor {
            f.set_cursor(x, y);
        }
    })?;
    // Apply cursor style after draw so ratatui's cursor-show/hide logic has run,
    // but the two are now in the same flush window (no visible frame gap).
    let cursor_style = if state.agents.artifacts_popup_open || state.focus == PaneId::Notes {
        SetCursorStyle::SteadyBar
    } else {
        match state.mode {
            Mode::Insert => SetCursorStyle::SteadyBar,
            Mode::Normal | Mode::Visual => SetCursorStyle::SteadyBlock,
        }
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

pub(super) fn render_prompt(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    message: &str,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title("CONFIRM")
        .style(Style::default().bg(theme.selection_bg));
    let paragraph = Paragraph::new(message)
        .style(Style::default().bg(theme.selection_bg).fg(theme.foreground))
        .block(block);
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

pub(super) fn render_command_prompt(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    message: &str,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title("COMMAND")
        .style(Style::default().bg(theme.selection_bg));
    let paragraph = Paragraph::new(message)
        .style(Style::default().bg(theme.selection_bg).fg(theme.foreground))
        .block(block);
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

pub(super) fn render_search_prompt(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    message: &str,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title("SEARCH")
        .style(Style::default().bg(theme.selection_bg));
    let paragraph = Paragraph::new(message)
        .style(Style::default().bg(theme.selection_bg).fg(theme.foreground))
        .block(block);
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}
