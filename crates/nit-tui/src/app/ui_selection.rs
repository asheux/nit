use arboard::Clipboard;
use nit_core::{AppKind, AppState, TerminalSelectRegion, UiSelection, UiSelectionPane, YankKind};

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

fn point_in_terminal_region(region: &TerminalSelectRegion, x: u16, y: u16) -> bool {
    x >= region.x
        && x < region.x.saturating_add(region.width)
        && y >= region.y
        && y < region.y.saturating_add(region.height)
}

/// The on-screen terminal grid under `(x, y)`, topmost first so a terminal
/// popup overlay wins over an inline terminal painted behind it. `None` when
/// the point is over no terminal. Used on mouse-down to start a selection.
pub(crate) fn terminal_region_at(
    state: &AppState,
    x: u16,
    y: u16,
) -> Option<&TerminalSelectRegion> {
    state
        .terminal_select_regions
        .iter()
        .rev()
        .find(|r| point_in_terminal_region(r, x, y))
}

/// The most-recently-rendered terminal region for `pane`. Used during a drag to
/// re-resolve the grid the gesture started on even after the cursor leaves it.
pub(crate) fn terminal_region_for_pane(
    state: &AppState,
    pane: UiSelectionPane,
) -> Option<&TerminalSelectRegion> {
    state
        .terminal_select_regions
        .iter()
        .rev()
        .find(|r| r.pane == pane)
}

/// Resolve a mouse-down point to a terminal selection start, respecting the
/// popup's modality: while the terminal popup is open only its grid is
/// selectable, never an inline terminal painted behind it. Returns the pane and
/// the `(line, col, lines)` start, or `None` when the point is over no
/// (currently selectable) terminal.
pub(crate) fn terminal_mouse_down_hit(
    state: &AppState,
    x: u16,
    y: u16,
) -> Option<(UiSelectionPane, usize, usize, Vec<String>)> {
    let region = if state.terminal_popup.visible {
        let region = terminal_region_for_pane(state, UiSelectionPane::TerminalPopup)?;
        if !point_in_terminal_region(region, x, y) {
            return None;
        }
        region
    } else {
        terminal_region_at(state, x, y)?
    };
    let pane = region.pane;
    let (line, col, lines) = map_terminal_region(region, x, y, false)?;
    Some((pane, line, col, lines))
}

/// Map a point to a `(line, col, lines)` triple inside a terminal region. With
/// `clamp = false` (mouse-down) the point must be inside the grid; with
/// `clamp = true` (drag) an out-of-bounds point is pinned to the nearest
/// row/column so a drag past the edge keeps extending the selection. `lines` is
/// the region's snapshot, returned for the shared `selection_text` slicer.
pub(crate) fn map_terminal_region(
    region: &TerminalSelectRegion,
    x: u16,
    y: u16,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if region.lines.is_empty() {
        return None;
    }
    if !clamp && !point_in_terminal_region(region, x, y) {
        return None;
    }
    let last_line = region.lines.len().saturating_sub(1);
    let line_idx = (y.saturating_sub(region.y) as usize).min(last_line);
    let line_len = region.lines[line_idx].chars().count();
    let col = (x.saturating_sub(region.x) as usize).min(line_len);
    Some((line_idx, col, region.lines.clone()))
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
    if state.terminal_popup.visible {
        // The terminal popup is a modal overlay: while it's open only its own
        // grid is selectable, never an inline terminal painted behind it.
        return Some(matches!(target, MouseSelectTarget::Ui(TerminalPopup)));
    }
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

#[cfg(test)]
mod terminal_selection_tests {
    use super::*;
    use nit_core::Buffer;
    use std::path::PathBuf;

    fn region(pane: UiSelectionPane, x: u16, y: u16, lines: &[&str]) -> TerminalSelectRegion {
        TerminalSelectRegion {
            pane,
            x,
            y,
            width: lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16,
            height: lines.len() as u16,
            lines: lines.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn state_with_regions(regions: Vec<TerminalSelectRegion>, popup_visible: bool) -> AppState {
        let mut state = AppState::new(
            PathBuf::from("/ws"),
            Buffer::empty("e", None),
            Buffer::empty("n", None),
        );
        state.terminal_select_regions = regions;
        state.terminal_popup.visible = popup_visible;
        state
    }

    #[test]
    fn map_terminal_region_maps_point_to_row_col() {
        let r = region(
            UiSelectionPane::Terminal,
            10,
            5,
            &["hello world", "second line"],
        );
        // row 6 → line 1 (y origin 5); col 13 → col 3 (x origin 10).
        let (line, col, lines) = map_terminal_region(&r, 13, 6, false).unwrap();
        assert_eq!((line, col), (1, 3));
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn map_terminal_region_rejects_outside_when_not_clamping() {
        let r = region(UiSelectionPane::Terminal, 10, 5, &["hello"]);
        assert!(map_terminal_region(&r, 0, 0, false).is_none());
    }

    #[test]
    fn map_terminal_region_clamps_edges_when_dragging() {
        let r = region(UiSelectionPane::Terminal, 10, 5, &["hello", "world!!"]);
        // Drag above-left of the grid pins to (0, 0).
        assert_eq!(map_terminal_region(&r, 0, 0, true).unwrap().0, 0);
        assert_eq!(map_terminal_region(&r, 0, 0, true).unwrap().1, 0);
        // Drag far past the bottom-right pins to the last row's end-of-line.
        let (line, col, _) = map_terminal_region(&r, 999, 999, true).unwrap();
        assert_eq!(line, 1);
        assert_eq!(col, "world!!".chars().count());
    }

    #[test]
    fn terminal_region_at_returns_topmost_overlay() {
        // Inline pushed first, popup last → popup wins for an overlapping point.
        let inline = region(UiSelectionPane::Terminal, 0, 0, &["aaaaa"]);
        let popup = region(UiSelectionPane::TerminalPopup, 0, 0, &["bbbbb"]);
        let state = state_with_regions(vec![inline, popup], true);
        assert_eq!(
            terminal_region_at(&state, 1, 0).unwrap().pane,
            UiSelectionPane::TerminalPopup
        );
    }

    #[test]
    fn terminal_mouse_down_hit_respects_popup_modality() {
        let inline = region(UiSelectionPane::Terminal, 0, 0, &["inline"]);
        let popup = region(UiSelectionPane::TerminalPopup, 20, 0, &["popup"]);
        let state = state_with_regions(vec![inline, popup], true);
        // Click over the inline terminal while the popup is open → no selection.
        assert!(terminal_mouse_down_hit(&state, 1, 0).is_none());
        // Click over the popup grid → popup selection.
        assert_eq!(
            terminal_mouse_down_hit(&state, 21, 0).unwrap().0,
            UiSelectionPane::TerminalPopup
        );
    }

    #[test]
    fn terminal_mouse_down_hit_resolves_inline_when_no_popup() {
        let inline = region(UiSelectionPane::Terminal, 0, 0, &["inline text"]);
        let state = state_with_regions(vec![inline], false);
        let (pane, line, col, _) = terminal_mouse_down_hit(&state, 3, 0).unwrap();
        assert_eq!(pane, UiSelectionPane::Terminal);
        assert_eq!((line, col), (0, 3));
    }

    #[test]
    fn selection_text_slices_terminal_lines() {
        let lines: Vec<String> = vec!["hello world".into(), "second line".into()];
        let selection = UiSelection {
            pane: UiSelectionPane::Terminal,
            start_line: 0,
            start_col: 6,
            end_line: 1,
            end_col: 6,
        };
        // "world" (col 6 → EOL of line 0) + "second" (col 0..6 of line 1).
        assert_eq!(selection_text(&lines, selection), "world\nsecond");
    }
}
