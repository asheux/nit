use arboard::Clipboard;
use nit_core::{AppKind, AppState, UiSelection, UiSelectionPane, YankKind};

use super::chat_input::slice_by_char;
use super::input_state::{InputState, MouseSelectAnchor, MouseSelectTarget, UiSelectionSignature};
use super::key_predicates::games_petri_visible;

pub(super) fn lines_to_strings(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

pub(super) fn update_ui_selection_text(
    state: &mut AppState,
    pane: UiSelectionPane,
    lines: &[String],
    clipboard: &mut Option<Clipboard>,
    input_state: &mut InputState,
) {
    let Some(selection) = state.ui_selection else {
        return;
    };
    if selection.pane != pane {
        return;
    }
    let signature = UiSelectionSignature {
        pane,
        start_line: selection.start_line,
        start_col: selection.start_col,
        end_line: selection.end_line,
        end_col: selection.end_col,
    };
    if input_state.last_ui_selection == Some(signature) {
        return;
    }
    input_state.last_ui_selection = Some(signature);
    let text = if matches!(pane, UiSelectionPane::AgentConsole) {
        selection_text_agent_console(lines, selection)
    } else {
        selection_text(lines, selection)
    };
    if text.is_empty() {
        return;
    }
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

pub(crate) fn selection_text(lines: &[String], selection: UiSelection) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let (start_line, start_col, end_line, end_col) =
        if (selection.start_line, selection.start_col) <= (selection.end_line, selection.end_col) {
            (
                selection.start_line,
                selection.start_col,
                selection.end_line,
                selection.end_col,
            )
        } else {
            (
                selection.end_line,
                selection.end_col,
                selection.start_line,
                selection.start_col,
            )
        };
    let mut out = String::new();
    let last_line = lines.len().saturating_sub(1);
    let end_line = end_line.min(last_line);
    for (line_idx, line) in lines
        .iter()
        .enumerate()
        .take(end_line.saturating_add(1))
        .skip(start_line)
    {
        let line_len = line.chars().count();
        let sel_start = if line_idx == start_line { start_col } else { 0 };
        let sel_end = if line_idx == end_line {
            end_col
        } else {
            line_len
        };
        let sel_start = sel_start.min(line_len);
        let sel_end = sel_end.min(line_len);
        out.push_str(&slice_by_char(line, sel_start, sel_end));
        if line_idx < end_line {
            out.push('\n');
        }
    }
    out
}

pub(super) fn selection_text_agent_console(lines: &[String], selection: UiSelection) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let (start_line, start_col, end_line, end_col) =
        if (selection.start_line, selection.start_col) <= (selection.end_line, selection.end_col) {
            (
                selection.start_line,
                selection.start_col,
                selection.end_line,
                selection.end_col,
            )
        } else {
            (
                selection.end_line,
                selection.end_col,
                selection.start_line,
                selection.start_col,
            )
        };
    let last_line = lines.len().saturating_sub(1);
    let end_line = end_line.min(last_line);
    let mut out_lines = Vec::new();
    for line_idx in start_line..=end_line {
        let line = &lines[line_idx];
        let line_len = line.chars().count();
        let mut sel_start = if line_idx == start_line { start_col } else { 0 };
        let mut sel_end = if line_idx == end_line {
            end_col
        } else {
            line_len
        };
        sel_start = sel_start.min(line_len);
        sel_end = sel_end.min(line_len);
        let slice = if let Some((payload_start, payload_end)) =
            user_prompt_payload_bounds_in_block(lines, line_idx)
        {
            let sel_start = sel_start.max(payload_start);
            let sel_end = sel_end.min(payload_end);
            slice_by_char(line, sel_start, sel_end)
                .trim_end_matches(' ')
                .to_string()
        } else {
            slice_by_char(line, sel_start, sel_end)
        };
        out_lines.push(slice);
    }
    out_lines.join("\n")
}

pub(super) const USER_PROMPT_INDENT: usize = 2;

pub(super) fn is_user_prompt_row(line: &str) -> bool {
    // User prompts are padded out to the full thread width so the background fills the row.
    // That makes them easy to detect for clipboard trimming: they start with the fixed indent and
    // end with spaces.
    line.starts_with("  ") && line.ends_with(' ')
}

pub(super) fn user_prompt_payload_bounds_in_block(
    lines: &[String],
    idx: usize,
) -> Option<(usize, usize)> {
    let line = lines.get(idx)?;
    if !is_user_prompt_row(line) {
        return None;
    }

    // Find the contiguous block of padded user rows that this line belongs to.
    let mut start = idx;
    while start > 0 && is_user_prompt_row(&lines[start - 1]) {
        start = start.saturating_sub(1);
    }
    let mut end = idx;
    while end + 1 < lines.len() && is_user_prompt_row(&lines[end + 1]) {
        end = end.saturating_add(1);
    }

    // Only treat the block as a user prompt if it contains the "You" label line.
    let has_label = (start..=end).any(|line_idx| lines[line_idx].trim() == "You");
    if !has_label {
        return None;
    }

    let len = line.chars().count();
    let start_col = USER_PROMPT_INDENT.min(len);
    Some((start_col, len))
}

pub(super) fn reset_ui_selection(state: &mut AppState, input_state: &mut InputState) {
    input_state.mouse_select_anchor = None;
    state.ui_selection = None;
    input_state.last_ui_selection = None;
}

pub(super) fn adjust_agent_console_drag_col(
    lines: &[String],
    anchor_line: usize,
    line_idx: usize,
    col: usize,
) -> usize {
    let Some(line) = lines.get(line_idx) else {
        return col;
    };
    let Some(payload_start) = user_bubble_payload_start_col(line) else {
        return col;
    };
    if col > payload_start {
        return col;
    }
    if line_idx > anchor_line {
        line.chars().count()
    } else if line_idx < anchor_line {
        0
    } else {
        col
    }
}

pub(super) fn user_bubble_payload_start_col(line: &str) -> Option<usize> {
    is_user_prompt_row(line).then_some(USER_PROMPT_INDENT)
}

pub(super) fn mouse_drag_allowed(state: &AppState, anchor: MouseSelectAnchor) -> bool {
    if state.command_line.is_some() || state.prompt.is_some() {
        return false;
    }
    if state.rule_picker.open || state.protocol_picker.open {
        return false;
    }
    if let Some(allowed) = drag_target_for_modal(state, anchor.target) {
        return allowed;
    }
    if games_petri_visible(state) {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesPetriDish)
        );
    }
    true
}

fn drag_target_for_modal(state: &AppState, target: MouseSelectTarget) -> Option<bool> {
    use UiSelectionPane::*;
    if state.agents.artifacts_popup_open {
        return Some(matches!(
            target,
            MouseSelectTarget::Ui(ArtifactsPopup) | MouseSelectTarget::PopupChatInput
        ));
    }
    if state.agents.global_archive_open {
        return Some(matches!(
            target,
            MouseSelectTarget::Ui(ArtifactsHistoryPopup)
        ));
    }
    if state.show_help {
        return Some(matches!(target, MouseSelectTarget::Ui(HelpPopup)));
    }
    if state.app_kind != AppKind::Games {
        return None;
    }
    let games = &state.games;
    if games.analysis.open {
        return Some(matches!(target, MouseSelectTarget::Ui(GamesAnalysisPopup)));
    }
    if games.run_browser.open {
        return Some(matches!(
            target,
            MouseSelectTarget::Ui(GamesRunBrowserPopup)
        ));
    }
    if games.replay.open {
        return Some(matches!(target, MouseSelectTarget::Ui(GamesReplayPopup)));
    }
    if games.strategy_inspect.open {
        return Some(matches!(target, MouseSelectTarget::Ui(GamesStrategyPopup)));
    }
    if games.tm_sim.open {
        return Some(matches!(
            target,
            MouseSelectTarget::Ui(GamesTmSimPopupLeft)
                | MouseSelectTarget::Ui(GamesTmSimPopupRight)
        ));
    }
    if games.ca_sim.open {
        return Some(matches!(
            target,
            MouseSelectTarget::Ui(GamesCaSimPopupLeft)
                | MouseSelectTarget::Ui(GamesCaSimPopupRight)
        ));
    }
    if games.match_history.open {
        return Some(matches!(
            target,
            MouseSelectTarget::Ui(GamesMatchHistoryPopup)
        ));
    }
    None
}
