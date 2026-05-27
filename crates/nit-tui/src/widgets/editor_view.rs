use crate::theme::Theme;
use nit_core::{find_matching_bracket, BracketMatch, Buffer, LineDiffStatus, Mode, PaneId};
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

/// Per-render search-highlight spec. `live` means a `/` prompt is open and
/// the term is being typed — `editor_view` paints those matches with a
/// stronger style on the cursor's own line so the operator can see which
/// hit `Enter` will commit to.
#[derive(Clone, Copy, Debug)]
pub struct SearchHighlight<'a> {
    pub term: &'a str,
    pub whole_word: bool,
    pub case_insensitive: bool,
    pub live: bool,
}

const EDITOR_TITLE: &str = "EDITOR  [ SAVE ]";
const DEFAULT_LINE_NUM_WIDTH: usize = 3;
const GUTTER_PAD_CHARS: usize = 4;

pub struct CursorPlacement {
    pub x: u16,
    pub y: u16,
}

struct LineData {
    content: String,
    chars: Vec<char>,
    base_style: Style,
    is_cursor_line: bool,
    mapped_segments: Option<Vec<nit_syntax::MappedLineSegment>>,
    diff_status: LineDiffStatus,
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
        EDITOR_TITLE,
        theme,
        tab_width,
        true,
        mode,
        None,
    )
}

/// Overlay a search highlight for `term` while a vim `/`, `*`, or `#`
/// search is active. Pass `SearchHighlight::live` to render the
/// "type-time" style — used while the `/` prompt is still open — which
/// matches every occurrence in the viewport and emphasises the cursor's
/// match more strongly than the committed-search style.
#[allow(clippy::too_many_arguments)]
pub fn render_editor_with_search(
    frame: &mut Frame,
    area: Rect,
    buffer: &Buffer,
    snapshot: Option<&HighlightSnapshot>,
    line_map: Option<&[Option<usize>]>,
    focus: PaneId,
    mode: Mode,
    theme: &Theme,
    tab_width: usize,
    search: Option<SearchHighlight<'_>>,
) -> Option<CursorPlacement> {
    render_buffer(
        frame,
        area,
        buffer,
        snapshot,
        line_map,
        PaneId::Editor,
        focus,
        EDITOR_TITLE,
        theme,
        tab_width,
        true,
        mode,
        search,
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
    search: Option<SearchHighlight<'_>>,
) -> Option<CursorPlacement> {
    let focused = focus == pane_id;
    let content_bg = buffer_input_bg(theme, focused);
    let block = build_editor_block(theme, focused, title);

    let actual_lines = buffer.lines_len();
    let total_lines = actual_lines.max(1);
    let line_num_width = total_lines.to_string().len().max(DEFAULT_LINE_NUM_WIDTH);
    let gutter_width = line_num_width + GUTTER_PAD_CHARS;
    let start = buffer.viewport.offset_line;
    let height = buffer.viewport.height.max(1);
    let content_width = buffer.viewport.width.max(1);

    let selection = buffer.selection_range();
    let selection_active = mode == Mode::Visual && selection.is_some();
    let diff_statuses = buffer.diff_statuses();

    let (line_data, highlight_error) = collect_line_data(
        buffer,
        snapshot,
        line_map,
        theme,
        content_bg,
        start,
        height,
        total_lines,
        diff_statuses,
        selection_active,
    );

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

    let bracket_pair = bracket_pair_for_render(buffer, &line_data, mode);
    let lines = render_lines(
        buffer,
        theme,
        content_bg,
        &line_data,
        start,
        total_lines,
        actual_lines,
        line_num_width,
        content_width,
        tab_width,
        selection,
        search,
        bracket_pair,
    );

    let paragraph = Paragraph::new(lines)
        .style(Style::default().bg(content_bg).fg(theme.foreground))
        .block(block);
    frame.render_widget(paragraph, area);

    if show_cursor && focused {
        Some(cursor_placement(
            buffer,
            area,
            start,
            gutter_width,
            tab_width,
        ))
    } else {
        None
    }
}

fn build_editor_block<'a>(theme: &Theme, focused: bool, title: &'a str) -> Block<'a> {
    let (border_color, border_type, title_color) = if focused {
        (theme.border_focused, BorderType::Thick, theme.title_focused)
    } else {
        (theme.border, BorderType::Plain, theme.title)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .border_type(border_type)
        .style(Style::default().bg(theme.background))
        .title(Span::styled(
            title,
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ))
}

#[allow(clippy::too_many_arguments)]
fn collect_line_data(
    buffer: &Buffer,
    snapshot: Option<&HighlightSnapshot>,
    line_map: Option<&[Option<usize>]>,
    theme: &Theme,
    content_bg: Color,
    start: usize,
    height: usize,
    total_lines: usize,
    diff_statuses: &[LineDiffStatus],
    selection_active: bool,
) -> (Vec<LineData>, Option<SegmentMapError>) {
    let mut line_data: Vec<LineData> = Vec::with_capacity(height);
    let mut highlight_error: Option<SegmentMapError> = None;

    for row in 0..height {
        let line_idx = start + row;
        let content = line_text_at(buffer, line_idx, total_lines);
        let is_cursor_line = line_idx == buffer.cursor.line;
        let base_style = base_line_style(theme, content_bg, is_cursor_line, selection_active);
        let chars: Vec<char> = content.chars().collect();
        let diff_status = diff_statuses
            .get(line_idx)
            .copied()
            .unwrap_or(LineDiffStatus::Unchanged);

        let mapped_segments = snapshot.and_then(|snap| {
            map_snapshot_for_line(snap, line_map, line_idx, &content, &mut highlight_error)
        });

        line_data.push(LineData {
            content,
            chars,
            base_style,
            is_cursor_line,
            mapped_segments,
            diff_status,
        });
    }

    (line_data, highlight_error)
}

fn line_text_at(buffer: &Buffer, line_idx: usize, total_lines: usize) -> String {
    if line_idx >= total_lines {
        return String::new();
    }
    let mut content = buffer.line_as_string(line_idx).replace('\r', "");
    if content.ends_with('\n') {
        content.pop();
    }
    content
}

fn base_line_style(
    theme: &Theme,
    content_bg: Color,
    is_cursor_line: bool,
    selection_active: bool,
) -> Style {
    let mut style = Style::default().fg(theme.foreground).bg(content_bg);
    if is_cursor_line && !selection_active {
        style = style
            .bg(theme.cursor_line_bg)
            .add_modifier(Modifier::UNDERLINED);
    }
    style
}

fn map_snapshot_for_line(
    snapshot: &HighlightSnapshot,
    line_map: Option<&[Option<usize>]>,
    line_idx: usize,
    content: &str,
    highlight_error: &mut Option<SegmentMapError>,
) -> Option<Vec<nit_syntax::MappedLineSegment>> {
    let mut snapshot_line = match line_map {
        Some(map) => map.get(line_idx).copied().flatten(),
        None => Some(line_idx),
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
    let snapshot_line = snapshot_line?;
    if let Some(hash) = snapshot.line_hashes.get(snapshot_line) {
        let hash_now = current_hash.unwrap_or_else(|| hash_line_bytes(content.as_bytes()));
        if *hash != hash_now {
            return None;
        }
    }
    let segments = snapshot.per_line.get(snapshot_line)?;
    match map_line_segments_to_chars(content, segments) {
        Ok(mapped) => Some(mapped),
        Err(err) => {
            *highlight_error = Some(err);
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_lines<'a>(
    buffer: &Buffer,
    theme: &Theme,
    content_bg: Color,
    line_data: &'a [LineData],
    start: usize,
    total_lines: usize,
    actual_lines: usize,
    line_num_width: usize,
    content_width: usize,
    tab_width: usize,
    selection: Option<(usize, usize)>,
    search: Option<SearchHighlight<'_>>,
    bracket_pair: Option<BracketRender>,
) -> Vec<Line<'a>> {
    let mut lines: Vec<Line> = Vec::with_capacity(line_data.len());
    for (row, data) in line_data.iter().enumerate() {
        let line_idx = start + row;
        let mut styles = vec![data.base_style; data.chars.len()];
        if let Some(mapped) = data.mapped_segments.as_ref() {
            apply_syntax_spans(mapped, &mut styles, theme);
        }
        apply_search_highlights(&mut styles, buffer, line_idx, actual_lines, theme, search);
        apply_selection_highlight(
            &mut styles,
            buffer,
            line_idx,
            actual_lines,
            selection,
            data.chars.len(),
            theme,
        );
        apply_bracket_pair_highlight(&mut styles, line_idx, bracket_pair, theme);

        let (ln_text, ln_style, sep_style) = gutter_styles(
            theme,
            content_bg,
            data,
            line_idx,
            total_lines,
            line_num_width,
        );
        let diff_sep_style = diff_sep_style(sep_style, data.diff_status, theme);
        let diff_indicator = match data.diff_status {
            LineDiffStatus::DeletedAbove => "▔",
            _ => "│",
        };

        let mut spans = vec![
            Span::styled(ln_text, ln_style),
            Span::styled(format!("{diff_indicator} "), diff_sep_style),
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
    lines
}

fn apply_search_highlights(
    styles: &mut [Style],
    buffer: &Buffer,
    line_idx: usize,
    actual_lines: usize,
    theme: &Theme,
    search: Option<SearchHighlight<'_>>,
) {
    let Some(highlight) = search else {
        return;
    };
    if highlight.term.is_empty() || line_idx >= actual_lines {
        return;
    }
    let matches = buffer.search_line_matches_opt(
        line_idx,
        highlight.term,
        highlight.whole_word,
        highlight.case_insensitive,
    );
    let cursor_on_line = highlight.live && line_idx == buffer.cursor.line;
    for (m_start, m_end) in matches {
        let is_current =
            cursor_on_line && buffer.cursor.col >= m_start && buffer.cursor.col < m_end;
        for idx in m_start..m_end.min(styles.len()) {
            let mut style = styles[idx]
                .bg(theme.selection_bg)
                .add_modifier(Modifier::BOLD);
            if is_current {
                // Reverse-video the active match so the operator can see
                // which hit `Enter` will commit when the `/` prompt is open.
                style = style.add_modifier(Modifier::REVERSED);
            }
            styles[idx] = style;
        }
    }
}

/// Per-frame bracket-pair locations in viewport-relative form. Both ends are
/// stored so the painter can flag whichever one is on-screen — either the
/// cursor's bracket (when it carries syntax-driven styling) or its partner
/// (which the syntax pass might color identically to surrounding code).
#[derive(Copy, Clone)]
struct BracketRender {
    cursor_line: usize,
    cursor_col: usize,
    partner_line: usize,
    partner_col: usize,
}

/// Decide whether to compute a bracket-pair highlight this frame and return
/// the buffer coordinates of both ends if so. Skips the scan in Visual mode
/// (selection styling already dominates that case) and when the cursor sits
/// inside a string/comment syntax segment — naive char matching there
/// produces noisy flicker inside `"({"` literals.
fn bracket_pair_for_render(
    buffer: &Buffer,
    line_data: &[LineData],
    mode: Mode,
) -> Option<BracketRender> {
    if mode == Mode::Visual {
        return None;
    }
    let cursor_line = buffer.cursor.line;
    let cursor_col = buffer.cursor.col;
    let cursor_row = cursor_line.checked_sub(buffer.viewport.offset_line)?;
    let cursor_data = line_data.get(cursor_row)?;
    if cursor_in_string_or_comment(cursor_data, cursor_col) {
        return None;
    }
    let pair: BracketMatch = find_matching_bracket(buffer, cursor_line, cursor_col)?;
    let (partner_line, partner_col) = buffer.char_to_point(pair.partner_idx)?;
    Some(BracketRender {
        cursor_line,
        cursor_col,
        partner_line,
        partner_col,
    })
}

fn cursor_in_string_or_comment(line: &LineData, col: usize) -> bool {
    use nit_syntax::HighlightGroup;
    let Some(segments) = line.mapped_segments.as_ref() else {
        return false;
    };
    segments.iter().any(|seg| {
        col >= seg.start
            && col < seg.end
            && matches!(
                seg.group,
                HighlightGroup::String
                    | HighlightGroup::Char
                    | HighlightGroup::Comment
                    | HighlightGroup::DocComment
            )
    })
}

fn apply_bracket_pair_highlight(
    styles: &mut [Style],
    line_idx: usize,
    pair: Option<BracketRender>,
    theme: &Theme,
) {
    let Some(pair) = pair else {
        return;
    };
    let ends = [
        (pair.cursor_line, pair.cursor_col),
        (pair.partner_line, pair.partner_col),
    ];
    for (target_line, col) in ends {
        if target_line != line_idx {
            continue;
        }
        let Some(slot) = styles.get_mut(col) else {
            continue;
        };
        *slot = slot
            .bg(theme.bracket_match)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    }
}

fn apply_selection_highlight(
    styles: &mut [Style],
    buffer: &Buffer,
    line_idx: usize,
    actual_lines: usize,
    selection: Option<(usize, usize)>,
    char_count: usize,
    theme: &Theme,
) {
    if line_idx >= actual_lines {
        return;
    }
    let Some((start, end)) = selection else {
        return;
    };
    let line_start = buffer.line_char_start(line_idx);
    let line_end = buffer.line_char_end(line_idx);
    if end <= line_start || start >= line_end {
        return;
    }
    let sel_start = start.saturating_sub(line_start).min(char_count);
    let sel_end = end.saturating_sub(line_start).min(char_count);
    if sel_end <= sel_start {
        return;
    }
    for idx in sel_start..sel_end.min(styles.len()) {
        styles[idx] = styles[idx].bg(theme.selection_bg);
    }
}

fn gutter_styles(
    theme: &Theme,
    content_bg: Color,
    data: &LineData,
    line_idx: usize,
    total_lines: usize,
    line_num_width: usize,
) -> (String, Style, Style) {
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
    let ln_text = if line_idx < total_lines {
        let ln = format!("{:>width$}", line_idx + 1, width = line_num_width);
        format!(" {ln} ")
    } else {
        let ln_blank = " ".repeat(line_num_width);
        format!(" {ln_blank} ")
    };
    (ln_text, ln_style, sep_style)
}

fn diff_sep_style(sep_style: Style, status: LineDiffStatus, theme: &Theme) -> Style {
    match status {
        LineDiffStatus::Added => sep_style.fg(theme.diff_added),
        LineDiffStatus::Modified => sep_style.fg(theme.diff_modified),
        LineDiffStatus::DeletedAbove => sep_style.fg(theme.diff_deleted),
        LineDiffStatus::Unchanged => sep_style,
    }
}

fn cursor_placement(
    buffer: &Buffer,
    area: Rect,
    start: usize,
    gutter_width: usize,
    tab_width: usize,
) -> CursorPlacement {
    let cursor_line = buffer.cursor.line.saturating_sub(start);
    let cursor_line_index = buffer.cursor.line.min(buffer.lines_len().saturating_sub(1));
    let line_str = buffer.line_as_string(cursor_line_index);
    let mut line = line_str.as_str();
    if line.ends_with('\n') {
        line = &line[..line.len().saturating_sub(1)];
    }
    let cursor_display_col = display_col_for_char_idx(line, buffer.cursor.col, tab_width);
    let offset_display = display_col_for_char_idx(line, buffer.viewport.offset_col, tab_width);
    let x =
        area.x + 1 + gutter_width as u16 + cursor_display_col.saturating_sub(offset_display) as u16;
    let y = area.y + 1 + cursor_line as u16;
    CursorPlacement { x, y }
}

fn buffer_input_bg(theme: &Theme, focused: bool) -> Color {
    let dim_pct = if focused { 78 } else { 88 };
    let mut bg = dim_bg_towards(theme.cursor_line_bg, theme.background, dim_pct);
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
#[path = "tests/editor_view.rs"]
mod tests;
