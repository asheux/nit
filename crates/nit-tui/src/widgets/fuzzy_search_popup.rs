use nit_core::{AppState, SearchMode, SearchResultFile, SearchResultMatch};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::fuzzy_preview_runner::PreviewModel;
use crate::theme::Theme;

const MIN_WIDTH: u16 = 68;
const MIN_HEIGHT: u16 = 22;

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = (screen.width.saturating_mul(80) / 100).max(MIN_WIDTH);
    let height = (screen.height.saturating_mul(76) / 100).max(MIN_HEIGHT);
    (width.min(screen.width), height.min(screen.height))
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    preview: Option<&PreviewModel>,
    preview_scroll_delta: i32,
) {
    frame.render_widget(Clear, area);
    let title = " NIT FUZZY SEARCH ";
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    render_header(frame, layout[0], state, theme);
    render_body(
        frame,
        layout[1],
        state,
        theme,
        preview,
        preview_scroll_delta,
    );
    render_footer(frame, layout[2], theme);
}

fn render_header(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let mode = match state.fuzzy_search.mode {
        SearchMode::Files => "[FILES]",
        SearchMode::Content => "[CONTENT]",
    };
    let mode_style = Style::default()
        .fg(theme.title)
        .add_modifier(Modifier::BOLD);
    let query_style = Style::default().fg(theme.foreground);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let status_style = if state.fuzzy_search.indexing || state.fuzzy_search.searching {
        Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.foreground)
    };

    let hidden = if state.fuzzy_search.show_hidden {
        "ON"
    } else {
        "OFF"
    };
    let ignored = if state.fuzzy_search.show_ignored {
        "ON"
    } else {
        "OFF"
    };

    let results = match state.fuzzy_search.mode {
        SearchMode::Files => state.fuzzy_search.file_results.len(),
        SearchMode::Content => state.fuzzy_search.match_results.len(),
    };
    let mut status = if !state.fuzzy_search.status_msg.is_empty() {
        state.fuzzy_search.status_msg.clone()
    } else {
        format!("{results} results")
    };
    if status.is_empty() {
        status = format!("{results} results");
    }

    let line = Line::from(vec![
        Span::styled(mode.to_string(), mode_style),
        Span::styled("  > ", dim_style),
        Span::styled(state.fuzzy_search.query.clone(), query_style),
        Span::styled(
            format!("  {status}  hidden:{hidden} ignored:{ignored}"),
            status_style,
        ),
    ]);
    frame.render_widget(
        Paragraph::new(vec![line]).style(Style::default().bg(theme.background)),
        area,
    );
}

fn render_body(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    preview: Option<&PreviewModel>,
    preview_scroll_delta: i32,
) {
    let halves = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    render_results(frame, halves[0], state, theme);
    render_preview(frame, halves[1], theme, preview, preview_scroll_delta);
}

fn render_footer(frame: &mut Frame, area: Rect, theme: &Theme) {
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let hint =
        "Enter open  Esc close  Tab mode  ↑↓ list  Ctrl+J/K list  Ctrl+↑/↓ preview  F2 hidden  F3 ignored  F5 refresh";
    let line = Line::from(Span::styled(hint, dim_style));
    frame.render_widget(
        Paragraph::new(vec![line]).style(Style::default().bg(theme.background)),
        area,
    );
}

fn render_results(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let title_style = Style::default()
        .fg(theme.title)
        .add_modifier(Modifier::BOLD);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(" RESULTS ", title_style))
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let height = inner.height as usize;
    if height == 0 {
        return;
    }

    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);

    match state.fuzzy_search.mode {
        SearchMode::Files => {
            let rows = &state.fuzzy_search.file_results;
            let (scroll, selected) = scroll_state(
                rows.len(),
                state.fuzzy_search.scroll_offset,
                state.fuzzy_search.selected,
                height,
            );
            let end = (scroll + height).min(rows.len());
            let mut lines: Vec<Line<'static>> = Vec::with_capacity(height);
            if rows.is_empty() {
                lines.push(Line::from(Span::styled("No matches.", dim_style)));
            } else {
                for (idx, item) in rows.iter().enumerate().take(end).skip(scroll) {
                    let selected_row = idx == selected;
                    lines.push(render_file_row(item, selected_row, theme));
                }
            }
            let paragraph = Paragraph::new(lines)
                .style(Style::default().bg(theme.background))
                .scroll((0, 0));
            frame.render_widget(paragraph, inner);
        }
        SearchMode::Content => {
            let rows = &state.fuzzy_search.match_results;
            let (scroll, selected) = scroll_state(
                rows.len(),
                state.fuzzy_search.scroll_offset,
                state.fuzzy_search.selected,
                height,
            );
            let end = (scroll + height).min(rows.len());
            let mut lines: Vec<Line<'static>> = Vec::with_capacity(height);
            if rows.is_empty() {
                lines.push(Line::from(Span::styled("No matches.", dim_style)));
            } else {
                for (idx, item) in rows.iter().enumerate().take(end).skip(scroll) {
                    let selected_row = idx == selected;
                    lines.push(render_match_row(item, selected_row, theme));
                }
            }
            let paragraph = Paragraph::new(lines)
                .style(Style::default().bg(theme.background))
                .scroll((0, 0));
            frame.render_widget(paragraph, inner);
        }
    }
}

fn render_preview(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    preview: Option<&PreviewModel>,
    preview_scroll_delta: i32,
) {
    let mut title = " PREVIEW ".to_string();
    let mut truncated = false;
    if let Some(p) = preview {
        truncated = p.truncated;
    }
    if truncated {
        title.push_str("[truncated] ");
    }

    let title_style = Style::default()
        .fg(theme.title)
        .add_modifier(Modifier::BOLD);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(title, title_style))
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);

    let Some(model) = preview else {
        let paragraph = Paragraph::new(vec![Line::from(Span::styled(
            "Loading preview...",
            dim_style,
        ))])
        .style(Style::default().bg(theme.background));
        frame.render_widget(paragraph, inner);
        return;
    };

    let height = inner.height as usize;
    let max_scroll = model.lines.len().saturating_sub(height.max(1));
    let desired = model.anchor_line.saturating_sub(height / 3);
    let base = desired.min(max_scroll) as i32;
    let scroll = (base + preview_scroll_delta).clamp(0, max_scroll as i32) as usize;

    let paragraph = Paragraph::new(model.lines.clone())
        .style(Style::default().bg(theme.background))
        .scroll((scroll as u16, 0));
    frame.render_widget(paragraph, inner);
}

fn render_file_row(item: &SearchResultFile, selected: bool, theme: &Theme) -> Line<'static> {
    let base_style = Style::default().fg(theme.foreground).bg(theme.background);
    let selected_style = base_style
        .bg(theme.selection_bg)
        .add_modifier(Modifier::BOLD);
    let match_style = Style::default()
        .fg(theme.accent)
        .bg(if selected {
            theme.selection_bg
        } else {
            theme.background
        })
        .add_modifier(Modifier::BOLD);

    let row_style = if selected { selected_style } else { base_style };
    let prefix = if selected { "› " } else { "  " };

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(prefix.to_string(), row_style));

    if item.matched_indices.is_empty() {
        spans.push(Span::styled(item.rel_path.clone(), row_style));
        return Line::from(spans);
    }

    let mut highlight = item.matched_indices.clone();
    highlight.sort_unstable();
    highlight.dedup();
    let mut hi_pos = 0usize;
    let mut cur_style = row_style;
    let mut buf = String::new();
    for (pos, ch) in item.rel_path.chars().enumerate() {
        let is_hit = highlight.get(hi_pos).copied().is_some_and(|v| v == pos);
        if is_hit {
            hi_pos += 1;
        }
        let next_style = if is_hit { match_style } else { row_style };
        if next_style != cur_style && !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut buf), cur_style));
            cur_style = next_style;
        } else if next_style != cur_style {
            cur_style = next_style;
        }
        buf.push(ch);
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, cur_style));
    }
    Line::from(spans)
}

fn render_match_row(item: &SearchResultMatch, selected: bool, theme: &Theme) -> Line<'static> {
    let base_style = Style::default().fg(theme.foreground).bg(theme.background);
    let selected_style = base_style
        .bg(theme.selection_bg)
        .add_modifier(Modifier::BOLD);
    let dim_style = Style::default()
        .fg(theme.border)
        .bg(if selected {
            theme.selection_bg
        } else {
            theme.background
        })
        .add_modifier(Modifier::DIM);
    let match_style = Style::default()
        .fg(theme.accent)
        .bg(if selected {
            theme.selection_bg
        } else {
            theme.background
        })
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);

    let row_style = if selected { selected_style } else { base_style };
    let prefix = if selected { "› " } else { "  " };

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(prefix.to_string(), row_style));
    spans.push(Span::styled(
        format!("{}:{}:{}  ", item.rel_path, item.line, item.col),
        dim_style,
    ));

    let snippet_len = item.snippet.chars().count();
    let start = item.match_start.min(snippet_len);
    let end = (item.match_start + item.match_len).min(snippet_len);
    spans.extend(split_snippet_spans(
        &item.snippet,
        start,
        end,
        row_style,
        match_style,
    ));
    Line::from(spans)
}

fn split_snippet_spans(
    text: &str,
    start: usize,
    end: usize,
    base: Style,
    highlight: Style,
) -> Vec<Span<'static>> {
    if start >= end {
        return vec![Span::styled(text.to_string(), base)];
    }
    let before = slice_by_char(text, 0, start);
    let mid = slice_by_char(text, start, end);
    let after = slice_by_char(text, end, text.chars().count());
    let mut spans = Vec::new();
    if !before.is_empty() {
        spans.push(Span::styled(before, base));
    }
    if !mid.is_empty() {
        spans.push(Span::styled(mid, highlight));
    }
    if !after.is_empty() {
        spans.push(Span::styled(after, base));
    }
    spans
}

fn slice_by_char(input: &str, start: usize, end: usize) -> String {
    if start >= end {
        return String::new();
    }
    let mut start_byte = None;
    let mut end_byte = None;
    for (count, (idx, _)) in input.char_indices().enumerate() {
        if count == start {
            start_byte = Some(idx);
        }
        if count == end {
            end_byte = Some(idx);
            break;
        }
    }
    let start_byte = start_byte.unwrap_or(input.len());
    let end_byte = end_byte.unwrap_or(input.len());
    input[start_byte..end_byte].to_string()
}

fn scroll_state(len: usize, scroll: usize, selected: usize, height: usize) -> (usize, usize) {
    if len == 0 || height == 0 {
        return (0, 0);
    }
    let selected = selected.min(len - 1);
    let mut scroll = scroll.min(len.saturating_sub(1));
    if selected < scroll {
        scroll = selected;
    } else if selected >= scroll + height {
        scroll = selected.saturating_sub(height - 1);
    }
    let max_scroll = len.saturating_sub(height);
    scroll = scroll.min(max_scroll);
    (scroll, selected)
}
