use crate::theme::Theme;
use nit_core::{Buffer, Mode, PaneId};
use nit_syntax::{hash_line_bytes, map_line_segments_to_chars, HighlightSnapshot, SegmentMapError};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthChar;

pub struct CursorPlacement {
    pub x: u16,
    pub y: u16,
}

#[allow(clippy::too_many_arguments)]
pub fn render_editor(
    frame: &mut Frame,
    area: Rect,
    buffer: &Buffer,
    snapshot: Option<&HighlightSnapshot>,
    line_map: Option<&[Option<usize>]>,
    focus: PaneId,
    mode: Mode,
    theme: &Theme,
    tab_width: usize,
) -> Option<CursorPlacement> {
    render_buffer(
        frame,
        area,
        buffer,
        snapshot,
        line_map,
        PaneId::Editor,
        focus,
        "EDITOR  [ SAVE ]",
        theme,
        tab_width,
        true,
        mode,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_buffer(
    frame: &mut Frame,
    area: Rect,
    buffer: &Buffer,
    snapshot: Option<&HighlightSnapshot>,
    line_map: Option<&[Option<usize>]>,
    pane_id: PaneId,
    focus: PaneId,
    title: &str,
    theme: &Theme,
    tab_width: usize,
    show_cursor: bool,
    mode: Mode,
) -> Option<CursorPlacement> {
    let focused = focus == pane_id;
    let content_bg = buffer_input_bg(theme, focused);
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
    let title_color = if focused {
        theme.title_focused
    } else {
        theme.title
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .border_type(border_type)
        .style(Style::default().bg(theme.background))
        .title(Span::styled(
            title,
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ));

    let actual_lines = buffer.lines_len();
    let total_lines = actual_lines.max(1);
    let line_num_width = total_lines.to_string().len().max(3);
    let gutter_width = line_num_width + 4;
    let start = buffer.viewport.offset_line;
    let height = buffer.viewport.height.max(1);
    let content_width = buffer.viewport.width.max(1);

    let selection = buffer.selection_range();
    let selection_active = mode == Mode::Visual && selection.is_some();
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    struct LineData {
        content: String,
        chars: Vec<char>,
        base_style: Style,
        is_cursor_line: bool,
        mapped_segments: Option<Vec<nit_syntax::MappedLineSegment>>,
    }

    let mut line_data: Vec<LineData> = Vec::with_capacity(height);
    let highlight_enabled = snapshot.is_some();
    let mut highlight_error: Option<SegmentMapError> = None;
    for row in 0..height {
        let line_idx = start + row;
        let mut content = if line_idx < total_lines {
            buffer.line_as_string(line_idx).replace('\r', "")
        } else {
            String::new()
        };
        if content.ends_with('\n') {
            content.pop();
        }
        let is_cursor_line = line_idx == buffer.cursor.line;
        let mut base_style = Style::default().fg(theme.foreground).bg(content_bg);
        if is_cursor_line && !selection_active {
            base_style = base_style
                .bg(theme.cursor_line_bg)
                .add_modifier(Modifier::UNDERLINED);
        }
        let chars: Vec<char> = content.chars().collect();

        let mapped_segments = if highlight_enabled {
            if let Some(snapshot) = snapshot {
                let mut snapshot_line = if let Some(map) = line_map {
                    map.get(line_idx).copied().flatten()
                } else {
                    Some(line_idx)
                };
                let mut current_hash: Option<u64> = None;
                if snapshot_line.is_none() {
                    if let Some(hash) = snapshot.line_hashes.get(line_idx) {
                        let hash_now = hash_line_bytes(content.as_bytes());
                        current_hash = Some(hash_now);
                        if *hash == hash_now {
                            snapshot_line = Some(line_idx);
                        }
                    }
                }
                if let Some(snapshot_line) = snapshot_line {
                    if let Some(hash) = snapshot.line_hashes.get(snapshot_line) {
                        let hash_now =
                            current_hash.unwrap_or_else(|| hash_line_bytes(content.as_bytes()));
                        if *hash != hash_now {
                            line_data.push(LineData {
                                content,
                                chars,
                                base_style,
                                is_cursor_line,
                                mapped_segments: None,
                            });
                            continue;
                        }
                    }
                    if let Some(segments) = snapshot.per_line.get(snapshot_line) {
                        match map_line_segments_to_chars(&content, segments) {
                            Ok(mapped) => Some(mapped),
                            Err(err) => {
                                highlight_error = Some(err);
                                None
                            }
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        line_data.push(LineData {
            content,
            chars,
            base_style,
            is_cursor_line,
            mapped_segments,
        });
    }
    if let Some(err) = highlight_error {
        log_rate_limited(&HIGHLIGHT_INVALID_SPAN_LOG, Duration::from_secs(1), || {
            tracing::warn!(
                start = err.start,
                end = err.end,
                line_len = err.line_len,
                "invalid highlight span; skipping highlight for line"
            );
        });
    }

    for (row, data) in line_data.iter().enumerate() {
        let line_idx = start + row;
        let mut styles = vec![data.base_style; data.chars.len()];
        if let Some(mapped) = data.mapped_segments.as_ref() {
            apply_syntax_spans(mapped, &mut styles, theme);
        }

        if line_idx < actual_lines {
            if let Some((sel_start, sel_end)) = selection.and_then(|(start, end)| {
                let line_start = buffer.line_char_start(line_idx);
                let line_end = buffer.line_char_end(line_idx);
                if end <= line_start || start >= line_end {
                    return None;
                }
                let mut sel_start = start.saturating_sub(line_start);
                let mut sel_end = end.saturating_sub(line_start);
                if sel_start > data.chars.len() {
                    sel_start = data.chars.len();
                }
                if sel_end > data.chars.len() {
                    sel_end = data.chars.len();
                }
                if sel_end <= sel_start {
                    None
                } else {
                    Some((sel_start, sel_end))
                }
            }) {
                for idx in sel_start..sel_end.min(styles.len()) {
                    styles[idx] = styles[idx].bg(theme.selection_bg);
                }
            }
        }

        let (ln_text, ln_style, sep_style) = if line_idx < total_lines {
            let ln = format!("{:>width$}", line_idx + 1, width = line_num_width);
            let gutter_bg = if data.is_cursor_line {
                Style::default().bg(theme.cursor_line_bg)
            } else {
                Style::default().bg(content_bg)
            };
            let ln_style = if data.is_cursor_line {
                Style::default()
                    .fg(theme.border_focused)
                    .add_modifier(Modifier::BOLD)
                    .patch(gutter_bg)
            } else {
                Style::default().fg(theme.border).patch(gutter_bg)
            };
            let sep_style = if data.is_cursor_line {
                Style::default().fg(theme.border_focused).patch(gutter_bg)
            } else {
                Style::default().fg(theme.border).patch(gutter_bg)
            };
            (format!(" {ln} "), ln_style, sep_style)
        } else {
            let gutter_bg = if data.is_cursor_line {
                Style::default().bg(theme.cursor_line_bg)
            } else {
                Style::default().bg(content_bg)
            };
            let ln_blank = " ".repeat(line_num_width);
            let ln_style = if data.is_cursor_line {
                Style::default()
                    .fg(theme.border_focused)
                    .add_modifier(Modifier::BOLD)
                    .patch(gutter_bg)
            } else {
                Style::default().fg(theme.border).patch(gutter_bg)
            };
            let sep_style = if data.is_cursor_line {
                Style::default().fg(theme.border_focused).patch(gutter_bg)
            } else {
                Style::default().fg(theme.border).patch(gutter_bg)
            };
            (format!(" {ln_blank} "), ln_style, sep_style)
        };

        let mut spans = vec![
            Span::styled(ln_text, ln_style),
            Span::styled("│ ", sep_style),
        ];

        let offset_display =
            display_col_for_char_idx(&data.content, buffer.viewport.offset_col, tab_width);
        spans.extend(build_spans(
            &data.chars,
            &styles,
            offset_display,
            content_width,
            tab_width,
            data.base_style,
        ));
        lines.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines)
        .style(Style::default().bg(content_bg).fg(theme.foreground))
        .block(block);

    frame.render_widget(paragraph, area);

    if show_cursor && focused {
        let cursor_line = buffer.cursor.line.saturating_sub(start);
        let cursor_line_index = buffer.cursor.line.min(buffer.lines_len().saturating_sub(1));
        let line_str = buffer.line_as_string(cursor_line_index);
        let mut line = line_str.as_str();
        if line.ends_with('\n') {
            line = &line[..line.len().saturating_sub(1)];
        }
        let cursor_display_col = display_col_for_char_idx(line, buffer.cursor.col, tab_width);
        let offset_display = display_col_for_char_idx(line, buffer.viewport.offset_col, tab_width);
        let x = area.x
            + 1
            + gutter_width as u16
            + cursor_display_col.saturating_sub(offset_display) as u16;
        let y = area.y + 1 + cursor_line as u16;
        return Some(CursorPlacement { x, y });
    }
    None
}

fn buffer_input_bg(theme: &Theme, focused: bool) -> Color {
    let mut bg = dim_bg_towards(
        theme.cursor_line_bg,
        theme.background,
        if focused { 78 } else { 88 },
    );
    if bg == theme.selection_bg {
        bg = theme.background;
    }
    if bg == theme.cursor_line_bg {
        bg = theme.background;
    }
    bg
}

fn dim_bg_towards(color: Color, background: Color, background_pct: u8) -> Color {
    let pct = background_pct.min(100) as u16;
    match (color, background) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r0, g0, b0)) => {
            let inv = 100u16.saturating_sub(pct);
            let mix = |top: u8, base: u8| -> u8 {
                let top = top as u16;
                let base = base as u16;
                ((top.saturating_mul(inv) + base.saturating_mul(pct) + 50) / 100) as u8
            };
            Color::Rgb(mix(r1, r0), mix(g1, g0), mix(b1, b0))
        }
        _ => color,
    }
}

fn apply_syntax_spans(
    segments: &[nit_syntax::MappedLineSegment],
    styles: &mut [Style],
    theme: &Theme,
) {
    for seg in segments {
        if seg.start >= seg.end || seg.start >= styles.len() {
            continue;
        }
        let style = theme.highlight_style(seg.group);
        for idx in seg.start..seg.end.min(styles.len()) {
            styles[idx] = styles[idx].patch(style);
        }
    }
}

fn display_col_for_char_idx(line: &str, char_idx: usize, tab_width: usize) -> usize {
    let mut col = 0;
    for (count, ch) in line.chars().enumerate() {
        if count >= char_idx {
            break;
        }
        if ch == '\t' {
            let tab = tab_width.max(1);
            let advance = tab - (col % tab);
            col += advance;
        } else {
            let w = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
            col += w;
        }
    }
    col
}

fn build_spans(
    chars: &[char],
    styles: &[Style],
    offset_col: usize,
    width: usize,
    tab_width: usize,
    base_style: Style,
) -> Vec<Span<'static>> {
    if chars.is_empty() {
        return vec![Span::styled(" ".repeat(width.max(1)), base_style)];
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_style = styles[0];
    let mut buffer = String::new();
    let mut col = 0;
    let visible_end = offset_col.saturating_add(width);

    let push_char = |ch: char,
                     style: Style,
                     spans: &mut Vec<Span<'static>>,
                     buffer: &mut String,
                     current_style: &mut Style| {
        if style != *current_style {
            if !buffer.is_empty() {
                spans.push(Span::styled(buffer.clone(), *current_style));
                buffer.clear();
            }
            *current_style = style;
        }
        buffer.push(ch);
    };

    for (idx, ch) in chars.iter().enumerate() {
        let style = styles[idx];
        if *ch == '\t' {
            let tab = tab_width.max(1);
            let advance = tab - (col % tab);
            for _ in 0..advance {
                if col >= visible_end {
                    break;
                }
                if col + 1 > offset_col {
                    push_char(' ', style, &mut spans, &mut buffer, &mut current_style);
                }
                col += 1;
            }
            continue;
        }

        let width = UnicodeWidthChar::width(*ch).unwrap_or(1).max(1);
        let start_col = col;
        let end_col = col + width;
        if end_col <= offset_col {
            col = end_col;
            continue;
        }
        if start_col >= visible_end {
            break;
        }
        push_char(*ch, style, &mut spans, &mut buffer, &mut current_style);
        col = end_col;
    }

    if !buffer.is_empty() {
        spans.push(Span::styled(buffer, current_style));
    }
    if spans.is_empty() {
        spans.push(Span::styled("", base_style));
    }
    spans.push(Span::styled(" ".repeat(width.max(1)), base_style));
    spans
}

fn log_rate_limited(lock: &'static OnceLock<Mutex<Instant>>, interval: Duration, f: impl FnOnce()) {
    let now = Instant::now();
    let guard = lock.get_or_init(|| Mutex::new(now - interval));
    let mut last = guard.lock().unwrap();
    if now.duration_since(*last) >= interval {
        *last = now;
        f();
    }
}

static HIGHLIGHT_INVALID_SPAN_LOG: OnceLock<Mutex<Instant>> = OnceLock::new();

#[cfg(test)]
mod tests {
    use super::*;
    use nit_syntax::{EngineKind, HighlightSnapshot, LanguageId, SyntaxStatus};
    use ratatui::style::Color;

    fn spans_to_string(spans: &[Span<'_>]) -> String {
        spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn build_spans_fills_trailing_spaces() {
        let chars: Vec<char> = "ab".chars().collect();
        let base = Style::default();
        let styles = vec![base; chars.len()];
        let spans = build_spans(&chars, &styles, 0, 6, 4, base);
        let rendered = spans_to_string(&spans);
        let visible: String = rendered.chars().take(6).collect();
        assert_eq!(&visible[..2], "ab");
        assert!(visible.chars().skip(2).all(|ch| ch == ' '));
    }

    #[test]
    fn tab_expands_with_style() {
        let chars: Vec<char> = "a\tb".chars().collect();
        let base = Style::default();
        let tab_style = Style::default().fg(Color::Red);
        let styles = vec![base, tab_style, base];
        let spans = build_spans(&chars, &styles, 0, 8, 4, base);
        let mut found = false;
        for span in &spans {
            let text = span.content.as_ref();
            if text.chars().all(|ch| ch == ' ') && text.len() == 3 {
                found = true;
                assert_eq!(span.style, tab_style);
            }
        }
        assert!(found, "expected tab expansion span with tab style");
    }

    #[test]
    fn plain_snapshot_version_matches() {
        let snapshot = HighlightSnapshot::plain(
            1,
            1,
            LanguageId::PlainText,
            EngineKind::Plain,
            SyntaxStatus::Ok(EngineKind::Plain),
            "",
        );
        assert_eq!(snapshot.version, 1);
    }
}
