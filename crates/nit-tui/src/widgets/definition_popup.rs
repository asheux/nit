//! Same-file goto-definition popup (`gd`). Renders the resolved definition
//! snippet as a scrollable, syntax-highlighted overlay. The highlight reuses
//! the editor's live `HighlightSnapshot`: v1 is same-file only, so the snippet
//! lines are buffer lines and no second parse is needed.

use nit_core::state::DefinitionView;
use nit_syntax::{map_line_segments_to_chars, HighlightSnapshot, MappedLineSegment};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::theme::Theme;

const TAB_WIDTH: usize = 4;
const FRAME_PADDING: u16 = 6;
const WIDTH_MAX: u16 = 100;
const HEIGHT_MAX: u16 = 30;

/// Popup dimensions: shrink for small terminals, clamp to readable bounds.
pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = screen
        .width
        .saturating_sub(FRAME_PADDING)
        .clamp(24, WIDTH_MAX);
    let height = screen
        .height
        .saturating_sub(FRAME_PADDING)
        .clamp(6, HEIGHT_MAX);
    (width, height)
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    view: &DefinitionView,
    snapshot: Option<&HighlightSnapshot>,
    line_map: Option<&[Option<usize>]>,
    theme: &Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title(Span::styled(
            view.title.clone(),
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(theme.background));
    let content = block.inner(area);

    let viewport = content.height as usize;
    let max_scroll = view.lines.len().saturating_sub(viewport);
    let offset = view.scroll.min(max_scroll);
    let window: Vec<Line<'static>> = view
        .lines
        .iter()
        .enumerate()
        .skip(offset)
        .take(viewport)
        .map(|(row, text)| {
            let buffer_line = view.start_line.saturating_sub(1) + row;
            let mapped =
                snapshot.and_then(|snap| mapped_for_line(snap, line_map, buffer_line, text));
            styled_line(text, mapped.as_deref(), theme)
        })
        .collect();

    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(window).style(Style::default().bg(theme.background).fg(theme.foreground)),
        content,
    );
}

/// Per-line highlight segments for `buffer_line`, translated through the
/// editor's current→snapshot `line_map` when the snapshot lags the buffer.
fn mapped_for_line(
    snapshot: &HighlightSnapshot,
    line_map: Option<&[Option<usize>]>,
    buffer_line: usize,
    text: &str,
) -> Option<Vec<MappedLineSegment>> {
    let snapshot_line = match line_map {
        Some(map) => map.get(buffer_line).copied().flatten()?,
        None => buffer_line,
    };
    let segments = snapshot.per_line.get(snapshot_line)?;
    map_line_segments_to_chars(text, segments).ok()
}

/// Flatten raw text + syntax segments into styled spans, expanding tabs so
/// ratatui's single-cell `\t` doesn't desync the popup grid.
fn styled_line(text: &str, mapped: Option<&[MappedLineSegment]>, theme: &Theme) -> Line<'static> {
    let base = Style::default().fg(theme.foreground).bg(theme.background);
    if text.is_empty() {
        return Line::from(Span::styled(String::new(), base));
    }
    let raw: Vec<char> = text.chars().collect();
    let mut styles = vec![base; raw.len()];
    if let Some(segments) = mapped {
        for seg in segments {
            let style = base.patch(theme.highlight_style(seg.group));
            for slot in styles
                .iter_mut()
                .take(seg.end.min(raw.len()))
                .skip(seg.start)
            {
                *slot = style;
            }
        }
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut run = styles[0];
    let mut col = 0usize;
    for (ch, style) in raw.into_iter().zip(styles) {
        if style != run {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), run));
            }
            run = style;
        }
        if ch == '\t' {
            let advance = TAB_WIDTH - (col % TAB_WIDTH);
            for _ in 0..advance {
                buf.push(' ');
            }
            col += advance;
        } else {
            buf.push(ch);
            col += 1;
        }
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, run));
    }
    Line::from(spans)
}
