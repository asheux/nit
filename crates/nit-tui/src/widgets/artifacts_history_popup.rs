use nit_core::{AppState, UiSelectionPane};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::theme::Theme;
use crate::widgets::agent_ops_view;
use crate::widgets::text_selection::apply_ui_selection;

const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 14;

/// Number of header lines before the scrollable entry list begins.
const HEADER_LINES: usize = 4;

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = (screen.width.saturating_mul(80) / 100)
        .clamp(MIN_WIDTH, 140)
        .min(screen.width);
    // Compact height: enough for search bar + header + entries + footer.
    let height = (screen.height.saturating_mul(65) / 100)
        .clamp(MIN_HEIGHT, 36)
        .min(screen.height);
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

pub fn entry_index_for_line(state: &AppState, line_idx: usize) -> Option<usize> {
    // Entries start right after HEADER_LINES (search, status, blank, column header).
    // Each entry is exactly one line with no separators.
    if line_idx < HEADER_LINES {
        return None;
    }
    let entry_idx = line_idx - HEADER_LINES;
    let count = state.agents.global_archive_filtered.len();
    (entry_idx < count).then_some(entry_idx)
}

fn kind_color(kind: &str, theme: &Theme) -> Color {
    match kind {
        "REPLY" => theme.success,
        "SYNTH" => theme.accent,
        "PLAN" => theme.title,
        "PATCH" => theme.warning,
        "EVIDENCE" => theme.title_focused,
        _ => theme.border, // PROMPT
    }
}

pub fn build_lines(state: &AppState, theme: &Theme, inner_width: u16) -> Vec<Line<'static>> {
    let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let selected_style = Style::default()
        .fg(theme.foreground)
        .bg(theme.selection_bg)
        .add_modifier(Modifier::BOLD);

    let max_width = inner_width.max(1) as usize;
    let index = &state.agents.global_archive_index;
    let filtered = &state.agents.global_archive_filtered;
    let total_count = index.len();
    let matching_count = filtered.len();
    let query = &state.agents.global_archive_query;
    let filter_label =
        agent_ops_view::saved_run_history_filter_label(state.agents.global_archive_filter);
    let selected = state
        .agents
        .global_archive_selected
        .min(matching_count.saturating_sub(1));

    let mut lines = Vec::new();

    // Line 0: search bar with visible block cursor.
    let cursor_style = Style::default().fg(theme.background).bg(theme.foreground);
    let prompt_style = Style::default()
        .fg(theme.title_focused)
        .add_modifier(Modifier::BOLD);
    if query.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(" / ", prompt_style),
            Span::styled(" ", cursor_style),
            Span::styled(" type to search...", dim_style),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled(" / ", prompt_style),
            Span::styled(query.clone(), value_style),
            Span::styled(" ", cursor_style),
        ]));
    }

    // Line 1: status bar.
    lines.push(Line::from(vec![
        Span::styled("filter: ", label_style),
        Span::styled(filter_label, value_style),
        Span::styled("  |  ", dim_style),
        Span::styled(format!("{total_count} artifacts"), value_style),
        Span::styled("  |  ", dim_style),
        Span::styled(format!("{matching_count} matching"), value_style),
    ]));

    // Line 2: blank separator.
    lines.push(Line::from(""));

    // Line 3: column header.
    let time_w = 14;
    let kind_w = 9;
    let owner_w = 30;
    // indent col = 3 chars for "↳ " or "  ", prefix = 2
    let fixed = 2 + 3 + kind_w + 1 + owner_w + 1 + time_w + 1;
    let preview_w = max_width.saturating_sub(fixed);
    let header = format!(
        "     {:<kind_w$} {:<owner_w$} {:<time_w$} PREVIEW",
        "KIND", "OWNER", "TIME",
    );
    lines.push(Line::from(Span::styled(
        trim_to_width(&header, max_width),
        dim_style,
    )));

    // Entries — prompts are top-level, replies/patches/evidence show with ↳.
    for (display_idx, &(_score, entry_idx)) in filtered.iter().enumerate() {
        let Some(entry) = index.get(entry_idx) else {
            continue;
        };
        let is_selected = display_idx == selected;
        let base_style = if is_selected {
            selected_style
        } else {
            value_style
        };

        let is_child = entry.kind != "PROMPT";
        let prefix = if is_selected { "> " } else { "  " };
        let indent = if is_child { "↳ " } else { "  " };

        let kind_label = match entry.kind {
            "PROMPT" => "PROMPT  ",
            "REPLY" => "REPLY   ",
            "SYNTH" => "SYNTH   ",
            "PLAN" => "PLAN    ",
            "PATCH" => "PATCH   ",
            "EVIDENCE" => "EVIDENCE",
            other => other,
        };
        let kind_span = Span::styled(
            kind_label.to_string(),
            if is_selected {
                selected_style
            } else {
                Style::default().fg(kind_color(entry.kind, theme))
            },
        );
        let owner_span = Span::styled(
            format!(" {:<owner_w$}", trim_to_width(&entry.owner, owner_w)),
            base_style,
        );
        let time_span = Span::styled(
            format!(" {:<time_w$}", trim_to_width(&entry.time_label, time_w)),
            if is_selected {
                selected_style
            } else {
                dim_style
            },
        );
        let preview_span = Span::styled(
            format!(" {}", trim_to_width(&entry.preview, preview_w)),
            base_style,
        );

        lines.push(Line::from(vec![
            Span::styled(prefix, base_style),
            Span::styled(
                indent,
                if is_selected {
                    selected_style
                } else {
                    dim_style
                },
            ),
            kind_span,
            owner_span,
            time_span,
            preview_span,
        ]));
    }

    if filtered.is_empty() {
        let msg = if query.is_empty() {
            "No archived artifacts found."
        } else {
            "No artifacts match your search."
        };
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(msg, dim_style)));
    }

    // Footer.
    lines.push(Line::from(""));
    let footer = if query.is_empty() {
        "Enter open | A all | D 24h | W 7d | M 30d | type to search | Esc close"
    } else {
        "Enter open | Backspace delete char | Ctrl+U clear | Esc clear/close"
    };
    lines.push(Line::from(Span::styled(footer, dim_style)));

    lines
}

pub fn render(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " GLOBAL ARTIFACT ARCHIVE ",
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
    let scroll = state.agents.global_archive_scroll.min(max_scroll);
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
