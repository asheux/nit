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
use crate::widgets::text_utils::truncate_text as trim_to_width;

const MIN_WIDTH: u16 = 80;
const MAX_WIDTH: u16 = 140;
const MIN_HEIGHT: u16 = 14;
const MAX_HEIGHT: u16 = 36;
const WIDTH_PCT: u16 = 80;
const HEIGHT_PCT: u16 = 65;

// Header rows (search, status, blank, column header) shown before the entry list.
const HEADER_LINES: usize = 4;
// Column widths for the entry table.
const KIND_W: usize = 9;
const OWNER_W: usize = 30;
const TIME_W: usize = 14;

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = (screen.width.saturating_mul(WIDTH_PCT) / 100)
        .clamp(MIN_WIDTH, MAX_WIDTH)
        .min(screen.width);
    // Compact height: enough for search bar + header + entries + footer.
    let height = (screen.height.saturating_mul(HEIGHT_PCT) / 100)
        .clamp(MIN_HEIGHT, MAX_HEIGHT)
        .min(screen.height);
    (width, height)
}

pub fn entry_index_for_line(state: &AppState, line_idx: usize) -> Option<usize> {
    // Each entry is exactly one line with no separators, starting after HEADER_LINES.
    let entry_idx = line_idx.checked_sub(HEADER_LINES)?;
    let count = state.agents.global_archive_filtered.len();
    (entry_idx < count).then_some(entry_idx)
}

fn kind_label_and_color(kind: &str, theme: &Theme) -> (&'static str, Color) {
    match kind {
        "PROMPT" => ("PROMPT  ", theme.border),
        "REPLY" => ("REPLY   ", theme.success),
        "SYNTH" => ("SYNTH   ", theme.accent),
        "PLAN" => ("PLAN    ", theme.title),
        "PATCH" => ("PATCH   ", theme.warning),
        "EVIDENCE" => ("EVIDENCE", theme.title_focused),
        _ => ("OTHER   ", theme.border),
    }
}

// Count rendered lines without building any styled line/span vectors. Called on the
// scroll hot path so wheel ticks don't re-iterate filtered entries + allocate styled
// spans just to compute `max_scroll`. Must stay in lock-step with `build_lines`.
pub fn line_count(state: &AppState) -> usize {
    let filtered = &state.agents.global_archive_filtered;
    let mut count = HEADER_LINES + filtered.len();
    if filtered.is_empty() {
        // blank separator + "no results" message
        count += 2;
    }
    // footer blank + footer text
    count + 2
}

pub fn build_lines(state: &AppState, theme: &Theme, inner_width: u16) -> Vec<Line<'static>> {
    let styles = PopupStyles::from_theme(theme);
    let max_width = inner_width.max(1) as usize;
    let index = &state.agents.global_archive_index;
    let filtered = &state.agents.global_archive_filtered;
    let query = &state.agents.global_archive_query;
    let filter_label =
        agent_ops_view::saved_run_history_filter_label(state.agents.global_archive_filter);
    let selected = state
        .agents
        .global_archive_selected
        .min(filtered.len().saturating_sub(1));

    let mut lines = Vec::new();
    lines.push(search_bar_line(query, &styles));
    lines.push(status_line(
        filter_label,
        index.len(),
        filtered.len(),
        &styles,
    ));
    lines.push(Line::from(""));

    // indent col = 3 chars for "↳ " or "  ", prefix = 2
    let fixed = 2 + 3 + KIND_W + 1 + OWNER_W + 1 + TIME_W + 1;
    let preview_w = max_width.saturating_sub(fixed);
    let header = format!(
        "     {:<KIND_W$} {:<OWNER_W$} {:<TIME_W$} PREVIEW",
        "KIND", "OWNER", "TIME",
    );
    lines.push(Line::from(Span::styled(
        trim_to_width(&header, max_width),
        styles.dim,
    )));

    // Entries — prompts are top-level, replies/patches/evidence show with ↳.
    for (display_idx, &(_score, entry_idx)) in filtered.iter().enumerate() {
        let Some(entry) = index.get(entry_idx) else {
            continue;
        };
        lines.push(entry_line(
            entry,
            display_idx == selected,
            preview_w,
            theme,
            &styles,
        ));
    }

    if filtered.is_empty() {
        let msg = if query.is_empty() {
            "No archived artifacts found."
        } else {
            "No artifacts match your search."
        };
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(msg, styles.dim)));
    }

    lines.push(Line::from(""));
    let footer = if query.is_empty() {
        "Enter open | A all | D 24h | W 7d | M 30d | type to search | Esc close"
    } else {
        "Enter open | Backspace delete char | Ctrl+U clear | Esc clear/close"
    };
    lines.push(Line::from(Span::styled(footer, styles.dim)));

    lines
}

struct PopupStyles {
    label: Style,
    value: Style,
    dim: Style,
    selected: Style,
    prompt: Style,
    cursor: Style,
}

impl PopupStyles {
    fn from_theme(theme: &Theme) -> Self {
        Self {
            label: Style::default().fg(theme.title).add_modifier(Modifier::DIM),
            value: Style::default().fg(theme.foreground),
            dim: Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
            selected: Style::default()
                .fg(theme.foreground)
                .bg(theme.selection_bg)
                .add_modifier(Modifier::BOLD),
            prompt: Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
            cursor: Style::default().fg(theme.background).bg(theme.foreground),
        }
    }
}

fn search_bar_line(query: &str, styles: &PopupStyles) -> Line<'static> {
    if query.is_empty() {
        Line::from(vec![
            Span::styled(" / ", styles.prompt),
            Span::styled(" ", styles.cursor),
            Span::styled(" type to search...", styles.dim),
        ])
    } else {
        Line::from(vec![
            Span::styled(" / ", styles.prompt),
            Span::styled(query.to_string(), styles.value),
            Span::styled(" ", styles.cursor),
        ])
    }
}

fn status_line(filter: &str, total: usize, matching: usize, styles: &PopupStyles) -> Line<'static> {
    Line::from(vec![
        Span::styled("filter: ", styles.label),
        Span::styled(filter.to_string(), styles.value),
        Span::styled("  |  ", styles.dim),
        Span::styled(format!("{total} artifacts"), styles.value),
        Span::styled("  |  ", styles.dim),
        Span::styled(format!("{matching} matching"), styles.value),
    ])
}

fn entry_line(
    entry: &nit_core::state::GlobalArchiveEntry,
    is_selected: bool,
    preview_w: usize,
    theme: &Theme,
    styles: &PopupStyles,
) -> Line<'static> {
    let base = if is_selected { styles.selected } else { styles.value };
    let tag = if is_selected { styles.selected } else { styles.dim };
    let prefix = if is_selected { "> " } else { "  " };
    let indent = if entry.kind == "PROMPT" { "  " } else { "↳ " };

    let (kind_label, kind_color) = kind_label_and_color(entry.kind, theme);
    let kind_style = if is_selected {
        styles.selected
    } else {
        Style::default().fg(kind_color)
    };

    Line::from(vec![
        Span::styled(prefix, base),
        Span::styled(indent, tag),
        Span::styled(kind_label, kind_style),
        Span::styled(padded_cell(&entry.owner, OWNER_W), base),
        Span::styled(padded_cell(&entry.time_label, TIME_W), tag),
        Span::styled(format!(" {}", trim_to_width(&entry.preview, preview_w)), base),
    ])
}

fn padded_cell(text: &str, width: usize) -> String {
    format!(" {:<width$}", trim_to_width(text, width))
}

/// Render the global artifacts archive popup, applying any active UI selection
/// so mouse-drag highlights line up with the scrolled viewport.
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
