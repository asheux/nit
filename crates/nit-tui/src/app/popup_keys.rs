#![allow(clippy::too_many_arguments)]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::{AppKind, AppState};

use crate::swarm::{chat_clone_base_id, SwarmRuntime};
use crate::theme::Theme;
use crate::widgets::{
    games_analysis_popup, games_ca_sim_popup, games_match_history_popup, games_replay_popup,
    games_run_browser_popup, games_strategy_popup, games_tm_sim_popup,
};

use super::*;

pub(super) fn handle_analysis_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> bool {
    if !state.games.analysis.open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    let (max_scroll, page_step) = games_analysis_popup_scroll_metrics(state, screen, theme);
    match key.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
            state.games.analysis.open = false;
            if let Some(selection) = state.ui_selection {
                if matches!(selection.pane, UiSelectionPane::GamesAnalysisPopup) {
                    state.ui_selection = None;
                }
            }
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            bump_scroll_clamped(&mut state.games.analysis.scroll_offset, -1, max_scroll);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            bump_scroll_clamped(&mut state.games.analysis.scroll_offset, 1, max_scroll);
            true
        }
        KeyCode::PageUp => {
            bump_scroll_clamped(
                &mut state.games.analysis.scroll_offset,
                -(page_step as i32),
                max_scroll,
            );
            true
        }
        KeyCode::PageDown => {
            bump_scroll_clamped(
                &mut state.games.analysis.scroll_offset,
                page_step as i32,
                max_scroll,
            );
            true
        }
        KeyCode::Home => {
            state.games.analysis.scroll_offset = 0;
            true
        }
        KeyCode::End => {
            state.games.analysis.scroll_offset = max_scroll;
            true
        }
        _ => true,
    }
}

pub(super) fn handle_run_browser_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
) -> bool {
    if state.app_kind != AppKind::Games || !state.games.run_browser.open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    let page_step = popup_page_step(popup_text_area(dynamic_popup_rect(
        screen,
        games_run_browser_popup::preferred_size(screen),
    )));
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.games.run_browser.open = false;
            if let Some(selection) = state.ui_selection {
                if matches!(selection.pane, UiSelectionPane::GamesRunBrowserPopup) {
                    state.ui_selection = None;
                }
            }
            true
        }
        KeyCode::Char('r') => {
            state.games.pending_run_browser = true;
            true
        }
        KeyCode::Enter => {
            if let Some(entry) = state
                .games
                .run_browser
                .entries
                .get(state.games.run_browser.selected)
            {
                state.games.pending_run_load = Some(entry.summary_path.clone());
                state.games.run_browser.loading = true;
            }
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if state.games.run_browser.selected > 0 {
                state.games.run_browser.selected -= 1;
            }
            adjust_run_browser_scroll(state, screen);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let max = state.games.run_browser.entries.len().saturating_sub(1);
            if state.games.run_browser.selected < max {
                state.games.run_browser.selected += 1;
            }
            adjust_run_browser_scroll(state, screen);
            true
        }
        KeyCode::PageUp => {
            state.games.run_browser.selected =
                state.games.run_browser.selected.saturating_sub(page_step);
            adjust_run_browser_scroll(state, screen);
            true
        }
        KeyCode::PageDown => {
            let max = state.games.run_browser.entries.len().saturating_sub(1);
            state.games.run_browser.selected =
                (state.games.run_browser.selected + page_step).min(max);
            adjust_run_browser_scroll(state, screen);
            true
        }
        _ => true,
    }
}

pub(super) fn handle_replay_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> bool {
    if state.app_kind != AppKind::Games || !state.games.replay.open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    let page_step = popup_page_step(popup_text_area(dynamic_popup_rect(
        screen,
        games_replay_popup::preferred_size(screen),
    )));
    let max_scroll = games_replay_popup_max_scroll(state, screen, theme);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.games.replay.open = false;
            if let Some(selection) = state.ui_selection {
                if matches!(selection.pane, UiSelectionPane::GamesReplayPopup) {
                    state.ui_selection = None;
                }
            }
            true
        }
        KeyCode::Char('r') => {
            state.games.replay.title = None;
            state.games.replay.lines.clear();
            state.games.replay.cycle = None;
            state.games.replay.scroll_offset = 0;
            true
        }
        KeyCode::Enter => {
            if state.games.replay.lines.is_empty() {
                let selection = games_replay_popup::pair_list(state)
                    .get(state.games.replay.selected_index)
                    .map(|(a, b)| (a.clone(), b.clone()));
                if let Some((a, b)) = selection {
                    state.games.pending_replay = Some(nit_core::GamesReplayRequest {
                        a_id: a.clone(),
                        b_id: b.clone(),
                    });
                    state.games.replay.selected_pair = Some((a.clone(), b.clone()));
                    state.games.replay.loading = true;
                    state.games.replay.last_error = None;
                }
            }
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if state.games.replay.lines.is_empty() {
                if state.games.replay.selected_index > 0 {
                    state.games.replay.selected_index -= 1;
                }
                adjust_replay_scroll(state, screen);
            } else {
                bump_scroll_clamped(&mut state.games.replay.scroll_offset, -1, max_scroll);
            }
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if state.games.replay.lines.is_empty() {
                let max = games_replay_popup::pair_list(state).len().saturating_sub(1);
                if state.games.replay.selected_index < max {
                    state.games.replay.selected_index += 1;
                }
                adjust_replay_scroll(state, screen);
            } else {
                bump_scroll_clamped(&mut state.games.replay.scroll_offset, 1, max_scroll);
            }
            true
        }
        KeyCode::PageUp => {
            if state.games.replay.lines.is_empty() {
                state.games.replay.selected_index =
                    state.games.replay.selected_index.saturating_sub(page_step);
                adjust_replay_scroll(state, screen);
            } else {
                bump_scroll_clamped(
                    &mut state.games.replay.scroll_offset,
                    -(page_step as i32),
                    max_scroll,
                );
            }
            true
        }
        KeyCode::PageDown => {
            if state.games.replay.lines.is_empty() {
                let max = games_replay_popup::pair_list(state).len().saturating_sub(1);
                state.games.replay.selected_index =
                    (state.games.replay.selected_index + page_step).min(max);
                adjust_replay_scroll(state, screen);
            } else {
                bump_scroll_clamped(
                    &mut state.games.replay.scroll_offset,
                    page_step as i32,
                    max_scroll,
                );
            }
            true
        }
        _ => true,
    }
}

pub(super) fn handle_strategy_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
) -> bool {
    if state.app_kind != AppKind::Games || !state.games.strategy_inspect.open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    let page_step = popup_page_step(popup_text_area(dynamic_popup_rect(
        screen,
        games_strategy_popup::preferred_size(screen),
    )));
    let max_scroll = games_strategy_popup_max_scroll(state, screen);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.games.strategy_inspect.open = false;
            if let Some(selection) = state.ui_selection {
                if matches!(selection.pane, UiSelectionPane::GamesStrategyPopup) {
                    state.ui_selection = None;
                }
            }
            true
        }
        KeyCode::Char('r') => {
            state.games.strategy_inspect.title = None;
            state.games.strategy_inspect.lines.clear();
            state.games.strategy_inspect.definition = None;
            state.games.strategy_inspect.scroll_offset = 0;
            true
        }
        KeyCode::Enter => {
            if state.games.strategy_inspect.lines.is_empty() {
                let defs = state.games.strategy_inspect.definitions.as_slice();
                if let Some(def) = defs.get(state.games.strategy_inspect.selected_index) {
                    state.games.strategy_inspect.title = Some(format!(
                        "{} — {}",
                        def.id,
                        games_visualizer_view::strategy_display_name_from_def(def)
                    ));
                    let mut lines = games_strategy_popup::build_definition_lines(def);
                    state.games.strategy_inspect.definition = Some(def.clone());
                    if state.games.strategy_inspect.source_label.as_deref() == Some("run") {
                        if let Some(run) = state.games.last_run.as_ref() {
                            if let Some(result) =
                                run.results.ranking.iter().find(|r| r.id == def.id)
                            {
                                if let Some(metrics) = result.tm_metrics.as_ref() {
                                    lines.push(String::new());
                                    lines.push("tm_metrics:".to_string());
                                    lines.push(format!(
                                        "avg_steps_per_move: {:.3}",
                                        metrics.avg_steps_per_move
                                    ));
                                    lines.push(format!(
                                        "min_steps_per_move: {}",
                                        metrics.min_steps_per_move
                                    ));
                                    lines.push(format!(
                                        "max_steps_per_move: {}",
                                        metrics.max_steps_per_move
                                    ));
                                    lines.push(format!(
                                        "max_steps_hit_count: {}",
                                        metrics.max_steps_hit_count
                                    ));
                                    lines.push(format!(
                                        "output_event_hit_rate: {:.3}",
                                        metrics.output_event_hit_rate
                                    ));
                                    lines.push(format!(
                                        "fallback_rate: {:.3}",
                                        metrics.fallback_rate
                                    ));
                                }
                            }
                        }
                    }
                    state.games.strategy_inspect.lines = lines;
                    state.games.strategy_inspect.scroll_offset = 0;
                }
            }
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if state.games.strategy_inspect.lines.is_empty() {
                if state.games.strategy_inspect.selected_index > 0 {
                    state.games.strategy_inspect.selected_index -= 1;
                }
                adjust_strategy_scroll(state, screen);
            } else {
                bump_scroll_clamped(
                    &mut state.games.strategy_inspect.scroll_offset,
                    -1,
                    max_scroll,
                );
            }
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if state.games.strategy_inspect.lines.is_empty() {
                let max = state
                    .games
                    .strategy_inspect
                    .definitions
                    .len()
                    .saturating_sub(1);
                if state.games.strategy_inspect.selected_index < max {
                    state.games.strategy_inspect.selected_index += 1;
                }
                adjust_strategy_scroll(state, screen);
            } else {
                bump_scroll_clamped(
                    &mut state.games.strategy_inspect.scroll_offset,
                    1,
                    max_scroll,
                );
            }
            true
        }
        KeyCode::PageUp => {
            if state.games.strategy_inspect.lines.is_empty() {
                state.games.strategy_inspect.selected_index = state
                    .games
                    .strategy_inspect
                    .selected_index
                    .saturating_sub(page_step);
                adjust_strategy_scroll(state, screen);
            } else {
                bump_scroll_clamped(
                    &mut state.games.strategy_inspect.scroll_offset,
                    -(page_step as i32),
                    max_scroll,
                );
            }
            true
        }
        KeyCode::PageDown => {
            if state.games.strategy_inspect.lines.is_empty() {
                let max = state
                    .games
                    .strategy_inspect
                    .definitions
                    .len()
                    .saturating_sub(1);
                state.games.strategy_inspect.selected_index =
                    (state.games.strategy_inspect.selected_index + page_step).min(max);
                adjust_strategy_scroll(state, screen);
            } else {
                bump_scroll_clamped(
                    &mut state.games.strategy_inspect.scroll_offset,
                    page_step as i32,
                    max_scroll,
                );
            }
            true
        }
        _ => true,
    }
}

pub(super) fn handle_tm_sim_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> bool {
    if state.app_kind != AppKind::Games || !state.games.tm_sim.open {
        return false;
    }
    if state.command_line.is_some() || state.prompt.is_some() {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    if is_command_prompt_open_key(key) {
        return false;
    }
    let page_step = popup_page_step(popup_text_area(dynamic_popup_rect(
        screen,
        games_tm_sim_popup::preferred_size(screen),
    )));
    let max_scroll = games_tm_sim_popup_max_scroll(state, screen, theme);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.games.tm_sim.open = false;
            // Reset the scroll cache so the next open starts fresh.
            state.games.tm_sim.last_max_scroll = usize::MAX;
            if let Some(selection) = state.ui_selection {
                if matches!(
                    selection.pane,
                    UiSelectionPane::GamesTmSimPopupLeft | UiSelectionPane::GamesTmSimPopupRight
                ) {
                    state.ui_selection = None;
                }
            }
            true
        }
        KeyCode::Char('r') => {
            state.games.tm_sim.scroll_offset = 0;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            bump_scroll_clamped(&mut state.games.tm_sim.scroll_offset, -1, max_scroll);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            bump_scroll_clamped(&mut state.games.tm_sim.scroll_offset, 1, max_scroll);
            true
        }
        KeyCode::PageUp => {
            bump_scroll_clamped(
                &mut state.games.tm_sim.scroll_offset,
                -(page_step as i32),
                max_scroll,
            );
            true
        }
        KeyCode::PageDown => {
            bump_scroll_clamped(
                &mut state.games.tm_sim.scroll_offset,
                page_step as i32,
                max_scroll,
            );
            true
        }
        _ => true,
    }
}

pub(super) fn handle_ca_sim_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> bool {
    if state.app_kind != AppKind::Games || !state.games.ca_sim.open {
        return false;
    }
    if state.command_line.is_some() || state.prompt.is_some() {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    if is_command_prompt_open_key(key) {
        return false;
    }
    let page_step = popup_page_step(popup_text_area(dynamic_popup_rect(
        screen,
        games_ca_sim_popup::preferred_size(screen),
    )));
    let max_scroll = games_ca_sim_popup_max_scroll(state, screen, theme);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.games.ca_sim.open = false;
            // Reset the scroll cache so the next open starts fresh.
            state.games.ca_sim.last_max_scroll = usize::MAX;
            if let Some(selection) = state.ui_selection {
                if matches!(
                    selection.pane,
                    UiSelectionPane::GamesCaSimPopupLeft | UiSelectionPane::GamesCaSimPopupRight
                ) {
                    state.ui_selection = None;
                }
            }
            true
        }
        KeyCode::Char('r') => {
            state.games.ca_sim.scroll_offset = 0;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            bump_scroll_clamped(&mut state.games.ca_sim.scroll_offset, -1, max_scroll);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            bump_scroll_clamped(&mut state.games.ca_sim.scroll_offset, 1, max_scroll);
            true
        }
        KeyCode::PageUp => {
            bump_scroll_clamped(
                &mut state.games.ca_sim.scroll_offset,
                -(page_step as i32),
                max_scroll,
            );
            true
        }
        KeyCode::PageDown => {
            bump_scroll_clamped(
                &mut state.games.ca_sim.scroll_offset,
                page_step as i32,
                max_scroll,
            );
            true
        }
        _ => true,
    }
}

pub(super) fn handle_match_history_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
) -> bool {
    if state.app_kind != AppKind::Games || !state.games.match_history.open {
        return false;
    }
    if state.command_line.is_some() || state.prompt.is_some() {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    if is_command_prompt_open_key(key) {
        return false;
    }
    let total = games_match_history_total_entries(state);
    let max_offset = games_match_history_max_offset(state, screen);
    let max_rounds = games_match_history_max_rounds(state);
    let default_rounds = games_match_history_default_rounds(state);
    let current_round_limit = state
        .games
        .match_history
        .round_limit
        .unwrap_or(default_rounds)
        .min(max_rounds);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.games.match_history.open = false;
            if let Some(selection) = state.ui_selection {
                if matches!(selection.pane, UiSelectionPane::GamesMatchHistoryPopup) {
                    state.ui_selection = None;
                }
            }
            true
        }
        KeyCode::Left | KeyCode::Char('h') => {
            state.games.match_history.column_offset =
                state.games.match_history.column_offset.saturating_sub(1);
            true
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if total > 0 {
                state.games.match_history.column_offset =
                    (state.games.match_history.column_offset + 1).min(max_offset);
            }
            true
        }
        KeyCode::PageUp => {
            state.games.match_history.column_offset =
                state.games.match_history.column_offset.saturating_sub(5);
            true
        }
        KeyCode::PageDown => {
            if total > 0 {
                state.games.match_history.column_offset =
                    (state.games.match_history.column_offset + 5).min(max_offset);
            }
            true
        }
        KeyCode::Home => {
            state.games.match_history.column_offset = 0;
            true
        }
        KeyCode::End => {
            if total > 0 {
                state.games.match_history.column_offset = max_offset;
            }
            true
        }
        KeyCode::Char('-') | KeyCode::Char('_') => {
            if max_rounds > 0 {
                let new_limit = current_round_limit.saturating_sub(10).max(1);
                state.games.match_history.round_limit = if new_limit == default_rounds {
                    None
                } else {
                    Some(new_limit)
                };
            }
            true
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            if max_rounds > 0 {
                let new_limit = current_round_limit.saturating_add(10).min(max_rounds);
                state.games.match_history.round_limit = if new_limit == default_rounds {
                    None
                } else {
                    Some(new_limit)
                };
            }
            true
        }
        KeyCode::Char('r') => {
            state.games.match_history.round_limit = None;
            true
        }
        _ => true,
    }
}

pub(super) fn maybe_follow_swarm_artifact_in_popup(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    focus: Option<&SwarmArtifactFocus>,
) {
    let Some(focus) = focus else {
        return;
    };
    if state.agents.artifacts_selected_saved_run_path.is_some() {
        return;
    }
    let mission_id = match focus {
        SwarmArtifactFocus::Task { mission_id, .. } => mission_id.as_str(),
        SwarmArtifactFocus::Report { mission_id } => mission_id.as_str(),
    };
    if state.agents.selected_context_mission() != Some(mission_id) {
        return;
    }

    let width = state.agents.ops_viewport_width.max(32);
    let card_idx = match focus {
        SwarmArtifactFocus::Task {
            mission_id,
            task_id,
        } => agent_ops_view::artifacts_card_index_for_swarm_task(
            state, swarm, width, mission_id, task_id,
        ),
        SwarmArtifactFocus::Report { mission_id } => {
            agent_ops_view::artifacts_card_index_for_swarm_report(state, swarm, width, mission_id)
        }
    };
    let Some(card_idx) = card_idx else {
        return;
    };

    state.agents.artifacts_selected = card_idx;
    state.agents.artifacts_popup_scroll = 0;
    if let Some(selection) = state.ui_selection {
        if matches!(selection.pane, UiSelectionPane::ArtifactsPopup) {
            state.ui_selection = None;
        }
    }
}

pub fn maybe_open_artifact_popup_from_console_line(
    state: &mut AppState,
    swarm: Option<&SwarmRuntime>,
    text_width: usize,
    line_idx: usize,
) -> bool {
    maybe_open_artifact_popup_from_console_line_for_pane(state, swarm, None, text_width, line_idx)
}

/// Pane-aware variant of [`maybe_open_artifact_popup_from_console_line`].
/// Multipane callers must pass `Some(pane_idx)` so the artifact-line
/// resolver walks the same pane-scoped message list the renderer used —
/// otherwise the row cursor drifts whenever an inline breather (e.g.
/// active shadow run) shifts subsequent rows and the click silently
/// misses.
pub fn maybe_open_artifact_popup_from_console_line_for_pane(
    state: &mut AppState,
    swarm: Option<&SwarmRuntime>,
    pane_idx: Option<usize>,
    text_width: usize,
    line_idx: usize,
) -> bool {
    let Some(message_idx) = agent_console_view::artifact_message_index_for_line_with_pane(
        state, swarm, pane_idx, text_width, line_idx,
    ) else {
        return false;
    };
    let Some(message) = state.agents.messages.get(message_idx).cloned() else {
        return false;
    };

    // Look up the card BEFORE mutating selected_agent/selected_mission.
    // Setting the context first would cause the clicked message itself to appear
    // as an artifact card, creating a self-fulfilling match.
    let selected =
        agent_ops_view::artifacts_popup_ref_for_message(state, swarm, text_width, message_idx)
            .and_then(|popup_ref| {
                agent_ops_view::artifacts_card_index_for_popup_ref(
                    state, swarm, text_width, &popup_ref,
                )
            });
    let Some(card_idx) = selected else {
        return false;
    };

    // Now that we know we're opening the popup, update the context.
    if let Some(mission_id) = message.mission_id.as_deref() {
        state.agents.selected_mission = Some(mission_id.to_string());
        if let Some(mission_idx) = state
            .agents
            .missions
            .iter()
            .position(|mission| mission.id == mission_id)
        {
            state.agents.mission_selected = mission_idx;
        }
    } else if let Some(agent_id) = message.agent_id.as_deref() {
        // Resolve chat-clone ids to the base agent so the context stays on the
        // user-selected model and other artifacts remain visible.
        let resolved = chat_clone_base_id(agent_id).unwrap_or(agent_id);
        state.agents.selected_mission = None;
        state.agents.selected_agent = Some(resolved.to_string());
    }

    state.agents.artifacts_selected_saved_run_path = None;
    state.agents.artifacts_selected = card_idx;
    state.agents.artifacts_popup_open = true;
    state.agents.artifacts_popup_scroll = 0;
    true
}

pub(super) fn recompute_global_archive_filter(state: &mut AppState) {
    state.agents.global_archive_filtered = agent_ops_view::filter_global_archive(
        &state.agents.global_archive_index,
        &state.agents.global_archive_query,
        state.agents.global_archive_filter,
    );
    state.agents.global_archive_selected = 0;
    state.agents.global_archive_scroll = 0;
}

pub(super) fn close_global_archive(state: &mut AppState) {
    state.agents.global_archive_open = false;
    state.agents.global_archive_scroll = 0;
    state.agents.global_archive_index.clear();
    state.agents.global_archive_filtered.clear();
    if let Some(selection) = state.ui_selection {
        if matches!(selection.pane, UiSelectionPane::ArtifactsHistoryPopup) {
            state.ui_selection = None;
        }
    }
}

pub(super) fn load_selected_global_archive_entry(state: &mut AppState) {
    let selected = state.agents.global_archive_selected;
    let Some(&(_, entry_idx)) = state.agents.global_archive_filtered.get(selected) else {
        return;
    };
    let Some(entry) = state.agents.global_archive_index.get(entry_idx).cloned() else {
        return;
    };

    // Store the entry so the artifact popup loads content directly from the
    // run.json.  We intentionally do NOT change selected_mission,
    // selected_agent, artifacts_selected_saved_run_path, or dock_tab — those
    // control the Evidence tab which should keep showing current-session
    // artifacts only.
    state.agents.global_archive_opened_entry = Some(entry.clone());

    // Open the artifact viewer popup on top of the RAG browser.
    state.agents.artifacts_popup_open = true;
    state.agents.artifacts_popup_scroll = 0;

    state.status = Some(format!("Artifact: {} ({})", entry.source, entry.time_label,));
}

pub(super) fn handle_global_archive_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> bool {
    if !state.agents.global_archive_open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }

    let query_empty = state.agents.global_archive_query.is_empty();
    let max = state.agents.global_archive_filtered.len().saturating_sub(1);
    let (_, page_step) = global_archive_scroll_metrics(state, screen, theme);

    match key.code {
        KeyCode::Esc => {
            if !query_empty {
                // First Esc clears the query.
                state.agents.global_archive_query.clear();
                state.agents.global_archive_query_cursor = 0;
                recompute_global_archive_filter(state);
            } else {
                close_global_archive(state);
            }
            true
        }
        KeyCode::Char('q') if query_empty => {
            close_global_archive(state);
            true
        }
        KeyCode::Enter => {
            load_selected_global_archive_entry(state);
            true
        }
        // Navigation: always available.
        KeyCode::Up => {
            state.agents.global_archive_selected =
                state.agents.global_archive_selected.saturating_sub(1);
            adjust_global_archive_scroll(state, screen, theme);
            true
        }
        KeyCode::Down => {
            state.agents.global_archive_selected =
                (state.agents.global_archive_selected + 1).min(max);
            adjust_global_archive_scroll(state, screen, theme);
            true
        }
        KeyCode::PageUp => {
            state.agents.global_archive_selected = state
                .agents
                .global_archive_selected
                .saturating_sub(page_step);
            adjust_global_archive_scroll(state, screen, theme);
            true
        }
        KeyCode::PageDown => {
            state.agents.global_archive_selected =
                (state.agents.global_archive_selected + page_step).min(max);
            adjust_global_archive_scroll(state, screen, theme);
            true
        }
        KeyCode::Home => {
            state.agents.global_archive_selected = 0;
            adjust_global_archive_scroll(state, screen, theme);
            true
        }
        KeyCode::End => {
            state.agents.global_archive_selected = max;
            adjust_global_archive_scroll(state, screen, theme);
            true
        }
        // Backspace: remove last char from query.
        KeyCode::Backspace => {
            if !query_empty {
                state.agents.global_archive_query.pop();
                state.agents.global_archive_query_cursor =
                    state.agents.global_archive_query.chars().count();
                recompute_global_archive_filter(state);
            }
            true
        }
        // Ctrl+U: clear query.
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.agents.global_archive_query.clear();
            state.agents.global_archive_query_cursor = 0;
            recompute_global_archive_filter(state);
            true
        }
        // Filter shortcuts: only when query is empty.
        KeyCode::Char('a') | KeyCode::Char('A') if query_empty => {
            state.agents.global_archive_filter = SavedRunHistoryFilter::All;
            recompute_global_archive_filter(state);
            true
        }
        KeyCode::Char('d') | KeyCode::Char('D') if query_empty => {
            state.agents.global_archive_filter = SavedRunHistoryFilter::LastDay;
            recompute_global_archive_filter(state);
            true
        }
        KeyCode::Char('w') | KeyCode::Char('W') if query_empty => {
            state.agents.global_archive_filter = SavedRunHistoryFilter::LastWeek;
            recompute_global_archive_filter(state);
            true
        }
        KeyCode::Char('m') | KeyCode::Char('M') if query_empty => {
            state.agents.global_archive_filter = SavedRunHistoryFilter::LastMonth;
            recompute_global_archive_filter(state);
            true
        }
        // All other printable chars: append to search query.
        KeyCode::Char(ch) => {
            state.agents.global_archive_query.push(ch);
            state.agents.global_archive_query_cursor =
                state.agents.global_archive_query.chars().count();
            recompute_global_archive_filter(state);
            true
        }
        _ => true,
    }
}

pub(super) fn handle_substrate_overlay_key(key: &KeyEvent, state: &mut AppState) -> bool {
    if !state.show_substrate_overlay {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    let max_scroll = state.substrate_overlay_last_max_scroll;
    let page_step: i32 = 10;

    match key.code {
        KeyCode::Esc | KeyCode::F(3) | KeyCode::Char('q') => {
            state.show_substrate_overlay = false;
            true
        }
        // Ctrl+Space also toggles the overlay closed.
        KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.show_substrate_overlay = false;
            true
        }
        KeyCode::Tab => {
            state.substrate_overlay_tab = match state.substrate_overlay_tab {
                nit_core::SubstrateOverlayTab::Signals => nit_core::SubstrateOverlayTab::Claims,
                nit_core::SubstrateOverlayTab::Claims => nit_core::SubstrateOverlayTab::Assumptions,
                nit_core::SubstrateOverlayTab::Assumptions => {
                    nit_core::SubstrateOverlayTab::Signals
                }
            };
            state.substrate_overlay_scroll = 0;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            bump_scroll_clamped(&mut state.substrate_overlay_scroll, -1, max_scroll);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            bump_scroll_clamped(&mut state.substrate_overlay_scroll, 1, max_scroll);
            true
        }
        KeyCode::PageUp => {
            bump_scroll_clamped(&mut state.substrate_overlay_scroll, -page_step, max_scroll);
            true
        }
        KeyCode::PageDown => {
            bump_scroll_clamped(&mut state.substrate_overlay_scroll, page_step, max_scroll);
            true
        }
        KeyCode::Home => {
            state.substrate_overlay_scroll = 0;
            true
        }
        KeyCode::End => {
            state.substrate_overlay_scroll = max_scroll;
            true
        }
        // Any other key is consumed so it cannot leak into editor/nittree
        // handlers while the overlay is open.
        _ => true,
    }
}

pub(super) fn handle_definition_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
) -> bool {
    if state.definition_popup.is_none() {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
        state.definition_popup = None;
        return true;
    }
    let area = dynamic_popup_rect(
        screen,
        crate::widgets::definition_popup::preferred_size(screen),
    );
    let viewport = popup_text_area(area).height as usize;
    let Some(view) = state.definition_popup.as_mut() else {
        return false;
    };
    let max_scroll = view.lines.len().saturating_sub(viewport);
    let page = viewport.max(1) as i32;
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => bump_scroll_clamped(&mut view.scroll, -1, max_scroll),
        KeyCode::Down | KeyCode::Char('j') => bump_scroll_clamped(&mut view.scroll, 1, max_scroll),
        KeyCode::PageUp => bump_scroll_clamped(&mut view.scroll, -page, max_scroll),
        KeyCode::PageDown => bump_scroll_clamped(&mut view.scroll, page, max_scroll),
        KeyCode::Home => view.scroll = 0,
        KeyCode::End => view.scroll = max_scroll,
        _ => {}
    }
    true
}

pub(super) fn handle_help_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> bool {
    if !state.show_help {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    let (max_scroll, page_step) = help_popup_scroll_metrics(screen, theme);
    let close = match key.code {
        KeyCode::Esc | KeyCode::F(1) | KeyCode::Enter | KeyCode::Char('q') => true,
        KeyCode::Char('?') if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            true
        }
        _ => false,
    };
    if close {
        state.show_help = false;
        state.help_scroll = 0;
        if let Some(selection) = state.ui_selection {
            if matches!(selection.pane, UiSelectionPane::HelpPopup) {
                state.ui_selection = None;
            }
        }
        true
    } else {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                bump_scroll_clamped(&mut state.help_scroll, -1, max_scroll);
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                bump_scroll_clamped(&mut state.help_scroll, 1, max_scroll);
                true
            }
            KeyCode::PageUp => {
                bump_scroll_clamped(&mut state.help_scroll, -(page_step as i32), max_scroll);
                true
            }
            KeyCode::PageDown => {
                bump_scroll_clamped(&mut state.help_scroll, page_step as i32, max_scroll);
                true
            }
            KeyCode::Home => {
                state.help_scroll = 0;
                true
            }
            KeyCode::End => {
                state.help_scroll = max_scroll;
                true
            }
            _ => true,
        }
    }
}

/// Outcome of a key pressed while the modal terminal popup owns input.
pub(crate) enum TerminalPopupKey {
    /// `Ctrl+Shift+T` — hide the popup; the shell keeps running.
    Close,
    /// Encoded bytes to forward to the shell stdin.
    Forward(Vec<u8>),
    /// Forward `bytes` to the shell AND hide the popup. Used by the
    /// double-Esc close affordance: the first Esc reaches vim/less/fzf
    /// inside the shell; the second within
    /// `chat_input::ESC_ESC_ABORT_WINDOW` (~500ms) also closes the popup.
    ForwardAndClose(Vec<u8>),
    /// A key with no terminal byte sequence — absorbed, never forwarded.
    Ignore,
}

// Per-thread tracker for the popup's double-Esc close affordance.
// Mirrors `chat_input::record_chat_esc_press`'s pattern — independent
// from chat's tracker so an Esc in chat then Esc in popup doesn't
// trigger a phantom close.
thread_local! {
    static LAST_POPUP_ESC_AT: std::cell::Cell<Option<std::time::Instant>>
        = const { std::cell::Cell::new(None) };
}

/// Returns true when an Esc key event lands within
/// `ESC_ESC_ABORT_WINDOW` of the previous one. Updates the timestamp
/// regardless. Caller resets via `clear_popup_esc_state` once the
/// close fires so a third Esc doesn't immediately re-trigger.
pub(crate) fn record_popup_esc_press() -> bool {
    let now = std::time::Instant::now();
    LAST_POPUP_ESC_AT.with(|cell| {
        let prev = cell.get();
        cell.set(Some(now));
        prev.is_some_and(|t| now.duration_since(t) <= super::chat_input::ESC_ESC_ABORT_WINDOW)
    })
}

pub(crate) fn clear_popup_esc_state() {
    LAST_POPUP_ESC_AT.with(|cell| cell.set(None));
}

/// `Ctrl+Shift+T`: the chord that both opens (`key_dispatch`) and closes
/// (here) the popup. Centralised so the two paths cannot diverge.
///
/// Two encoding schemes need to match because terminals vary on how
/// they report Shift for letter chords:
///
///   1. Modern (kitty keyboard protocol / crossterm DISAMBIGUATE
///      enhancements): modifiers contains BOTH CONTROL and SHIFT,
///      code is `Char('t')` or `Char('T')`.
///   2. Legacy (macOS Terminal.app, plain xterm-256color): modifiers
///      contains only CONTROL, code is `Char('T')` — uppercase IS
///      the shift signal.
///
/// Accepting both keeps `Ctrl+Shift+T` working on every terminal we
/// support. The lower-case `Ctrl+t` (no shift) is deliberately NOT
/// matched — that's an unrelated chord operators may want for
/// future bindings.
pub(crate) fn is_terminal_popup_toggle_key(key: &KeyEvent) -> bool {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }
    let shift_explicit = key.modifiers.contains(KeyModifiers::SHIFT);
    let shift_implicit_via_uppercase = matches!(key.code, KeyCode::Char('T'));
    if !(shift_explicit || shift_implicit_via_uppercase) {
        return false;
    }
    matches!(key.code, KeyCode::Char('t') | KeyCode::Char('T'))
}

/// Route a key while the terminal popup is focused. Three close
/// affordances exist; everything else encodes to PTY bytes:
///
///   * `Ctrl+Shift+T` — direct close, no PTY forwarding.
///   * `Esc Esc` within ~500ms — first Esc reaches the shell so vim /
///     less / fzf can react, second Esc also closes the popup.
///   * Click outside the popup rect — handled in the runner's mouse
///     branch, not here.
pub(crate) fn terminal_popup_key(key: &KeyEvent) -> TerminalPopupKey {
    if is_terminal_popup_toggle_key(key) {
        clear_popup_esc_state();
        return TerminalPopupKey::Close;
    }
    if matches!(key.code, KeyCode::Esc) {
        let bytes = crate::pty::encode_key(*key).unwrap_or_else(|| b"\x1b".to_vec());
        let double_tap = record_popup_esc_press();
        if double_tap {
            clear_popup_esc_state();
            return TerminalPopupKey::ForwardAndClose(bytes);
        }
        return TerminalPopupKey::Forward(bytes);
    }
    // Any other key in the popup resets the double-Esc tracker so a
    // stray Esc minutes ago can't pair with a fresh one to close.
    clear_popup_esc_state();
    match crate::pty::encode_key(*key) {
        Some(bytes) => TerminalPopupKey::Forward(bytes),
        None => TerminalPopupKey::Ignore,
    }
}
