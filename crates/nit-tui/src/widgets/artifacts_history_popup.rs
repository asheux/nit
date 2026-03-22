use nit_core::{AppState, SavedRunHistoryPendingAction, UiSelectionPane};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::theme::Theme;
use crate::widgets::agent_ops_view::{self, SavedArtifactsRunKind};
use crate::widgets::text_selection::apply_ui_selection;

const MIN_WIDTH: u16 = 72;
const MIN_HEIGHT: u16 = 16;

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = screen.width.clamp(MIN_WIDTH, 120);
    let height = screen.height.clamp(MIN_HEIGHT, 28);
    (width, height)
}

fn trim_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text.chars().count() <= max_width {
        return text.to_string();
    }
    let mut out = text
        .chars()
        .take(max_width.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}

fn entry_start_line() -> usize {
    6
}

pub fn entry_index_for_line(state: &AppState, line_idx: usize) -> Option<usize> {
    let entries = agent_ops_view::artifacts_history_visible_entries(state);
    if line_idx < entry_start_line() {
        return None;
    }
    let entry_idx = line_idx - entry_start_line();
    (entry_idx < entries.len()).then_some(entry_idx)
}

pub fn build_lines(state: &AppState, theme: &Theme, inner_width: u16) -> Vec<Line<'static>> {
    let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let warning_style = Style::default()
        .fg(theme.warning)
        .add_modifier(Modifier::BOLD);
    let selected_style = Style::default()
        .fg(theme.foreground)
        .bg(theme.selection_bg)
        .add_modifier(Modifier::BOLD);
    let active_style = Style::default()
        .fg(theme.title_focused)
        .add_modifier(Modifier::BOLD);

    let max_width = inner_width.max(1) as usize;
    let entries = agent_ops_view::artifacts_history_visible_entries(state);
    let archived_visible = entries.len().saturating_sub(1);
    let archived_total = agent_ops_view::artifacts_history_entries(state)
        .len()
        .saturating_sub(1);
    let selected = state
        .agents
        .artifacts_history_selected
        .min(entries.len().saturating_sub(1));
    let current_source = agent_ops_view::artifacts_history_summary_label(state);

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("source: ", label_style),
        Span::styled(current_source, active_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled("filter: ", label_style),
        Span::styled(
            agent_ops_view::saved_run_history_filter_label(state.agents.artifacts_history_filter),
            value_style,
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("saved runs: ", label_style),
        Span::styled(
            format!("{archived_visible}/{archived_total} visible"),
            value_style,
        ),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Select a saved run to load it into the Artifacts tab.",
        dim_style,
    )));
    let pending_line = match state.agents.artifacts_history_pending_action {
        Some(SavedRunHistoryPendingAction::DeleteSelected) => {
            let label = entries
                .get(selected)
                .map(|entry| entry.label.as_str())
                .unwrap_or("selected saved run");
            Line::from(Span::styled(
                format!("Confirm delete: press X again to remove {label} · Esc cancel"),
                warning_style,
            ))
        }
        Some(SavedRunHistoryPendingAction::PruneFiltered) => {
            let count = agent_ops_view::artifacts_history_prunable_entries(state).len();
            let filter = agent_ops_view::saved_run_history_filter_label(
                state.agents.artifacts_history_filter,
            );
            Line::from(Span::styled(
                format!("Confirm prune: press P again to remove {count} saved runs in {filter} · Esc cancel"),
                warning_style,
            ))
        }
        None => Line::from(Span::styled(
            "X delete selected · P prune current filter",
            dim_style,
        )),
    };
    lines.push(pending_line);

    for (idx, entry) in entries.iter().enumerate() {
        let style = if idx == selected {
            selected_style
        } else if (matches!(entry.kind, SavedArtifactsRunKind::Current)
            && state.agents.artifacts_selected_saved_run_path.is_none())
            || (matches!(entry.kind, SavedArtifactsRunKind::Archived)
                && entry.run_path.as_deref()
                    == state.agents.artifacts_selected_saved_run_path.as_deref())
        {
            active_style
        } else {
            value_style
        };
        let prefix = if idx == selected { "›" } else { " " };
        let text = format!(
            "{prefix} {}  {}",
            trim_to_width(entry.label.as_str(), max_width.saturating_sub(6)),
            trim_to_width(entry.detail.as_str(), max_width.saturating_sub(12)),
        );
        lines.push(Line::from(Span::styled(text, style)));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Enter load · X delete · P prune · A all · D 24h · W 7d · M 30d · R current/latest · Esc close",
        dim_style,
    )));

    lines
}

pub fn render(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " SAVED RUN HISTORY ",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = build_lines(state, theme, inner.width);
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    let scroll = state.agents.artifacts_history_popup_scroll.min(max_scroll);
    lines = apply_ui_selection(
        lines,
        state.ui_selection.as_ref(),
        UiSelectionPane::ArtifactsHistoryPopup,
        theme.selection_bg,
        scroll,
    );
    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));
    frame.render_widget(paragraph, inner);
}
