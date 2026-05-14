use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use nit_syntax::map_line_segments_to_chars;

use crate::theme::Theme;

use super::syntax::{canonical_code_lang, highlight_code_block, is_json_code_lang};
use super::table::{flush_markdown_table, is_markdown_table_candidate};
use super::{
    dim_bg_towards, popup_note_line, popup_rule_line, render_numbered_code_line, styled_math_spans,
    styled_text_spans, wrap_visual_line,
};

/// Render a markdown blob into ratatui `Line`s — fenced-code blocks pick
/// up tree-sitter highlighting via [`super::syntax`], tables fall through
/// to [`super::table`], math blocks render with a heavier accent. A top-
/// level JSON blob short-circuits to a pretty-printed code block so the
/// rendering UI doesn't choke on raw JSON dumps.
pub(super) fn render_markdown_document(text: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    if let Some(json_lines) = maybe_render_json_document(text, theme, width) {
        return json_lines;
    }

    let mut out = Vec::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_lines: Vec<String> = Vec::new();
    let mut math_block_end: Option<&'static str> = None;
    let mut math_lines: Vec<String> = Vec::new();
    let mut table_lines: Vec<String> = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();

        if in_code_block {
            if parse_code_fence(trimmed).is_some() {
                out.extend(render_fenced_code_block(
                    code_lang.as_str(),
                    code_lines.as_slice(),
                    theme,
                    width,
                ));
                out.push(Line::from(""));
                in_code_block = false;
                code_lang.clear();
                code_lines.clear();
            } else {
                code_lines.push(line.to_string());
            }
            continue;
        }

        if let Some(end_marker) = math_block_end {
            if trimmed == end_marker {
                out.extend(render_math_block(math_lines.as_slice(), theme, width));
                out.push(Line::from(""));
                math_block_end = None;
                math_lines.clear();
            } else {
                math_lines.push(line.to_string());
            }
            continue;
        }

        if !in_code_block && is_markdown_table_candidate(trimmed) {
            table_lines.push(trimmed.to_string());
            continue;
        }
        if !table_lines.is_empty() {
            flush_markdown_table(&mut out, &mut table_lines, theme, width);
        }

        if let Some(lang) = parse_code_fence(trimmed) {
            in_code_block = true;
            code_lang = lang.to_string();
            code_lines.clear();
            continue;
        }

        if let Some(end_marker) = parse_math_block_start(trimmed) {
            math_block_end = Some(end_marker);
            math_lines.clear();
            continue;
        }
        if let Some(single_line_math) = extract_single_line_math_block(trimmed) {
            out.extend(render_math_block(&[single_line_math], theme, width));
            out.push(Line::from(""));
            continue;
        }

        if trimmed.is_empty() {
            out.push(Line::from(""));
            continue;
        }
        if is_thematic_rule(trimmed) {
            out.push(popup_rule_line(width, theme));
            continue;
        }
        if let Some((level, heading)) = parse_markdown_heading(trimmed) {
            out.extend(render_markdown_heading(level, heading, theme, width));
            continue;
        }
        if let Some(heading) = strong_only_heading_text(trimmed) {
            out.extend(render_markdown_heading(2, &heading, theme, width));
            continue;
        }
        if let Some(quote) = trimmed.strip_prefix('>').map(str::trim_start) {
            out.extend(render_markdown_quote(quote, theme, width));
            continue;
        }
        if let Some((marker, item)) = parse_list_marker(trimmed) {
            out.extend(render_markdown_list_item(marker, item, theme, width));
            continue;
        }
        out.extend(render_markdown_paragraph(trimmed, theme, width));
    }

    if in_code_block {
        out.extend(render_fenced_code_block(
            code_lang.as_str(),
            code_lines.as_slice(),
            theme,
            width,
        ));
    }
    if math_block_end.is_some() {
        out.extend(render_math_block(math_lines.as_slice(), theme, width));
    }
    if !table_lines.is_empty() {
        flush_markdown_table(&mut out, &mut table_lines, theme, width);
    }
    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

fn maybe_render_json_document(
    text: &str,
    theme: &Theme,
    width: usize,
) -> Option<Vec<Line<'static>>> {
    let trimmed = text.trim();
    if !matches!(trimmed.chars().next(), Some('{') | Some('[')) {
        return None;
    }
    let value = serde_json::from_str::<serde_json::Value>(trimmed).ok()?;
    let pretty = serde_json::to_string_pretty(&value).ok()?;

    let mut out = Vec::new();
    out.push(popup_note_line(" json document", theme.accent, theme));
    out.extend(render_fenced_code_block(
        "json",
        &pretty.lines().map(str::to_string).collect::<Vec<_>>(),
        theme,
        width,
    ));
    Some(out)
}

fn render_markdown_heading(
    level: usize,
    heading: &str,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let (prefix, style) = match level {
        1 => (
            " § ",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ),
        2 => (
            " • ",
            Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::BOLD),
        ),
        _ => (
            " · ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
    };
    let available = width.saturating_sub(UnicodeWidthStr::width(prefix)).max(8);
    wrap_visual_line(heading, available)
        .into_iter()
        .enumerate()
        .map(|(idx, segment)| {
            let leading = if idx == 0 {
                Span::styled(prefix.to_string(), style)
            } else {
                Span::styled(" ".repeat(UnicodeWidthStr::width(prefix)), style)
            };
            Line::from(vec![leading, Span::styled(segment, style)])
        })
        .collect()
}

fn render_markdown_quote(text: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let prefix = " │ ";
    let available = width.saturating_sub(UnicodeWidthStr::width(prefix)).max(8);
    let quote_style = Style::default()
        .fg(theme.border_focused)
        .add_modifier(Modifier::ITALIC);
    wrap_visual_line(text, available)
        .into_iter()
        .map(|segment| {
            let mut spans = vec![Span::styled(
                prefix.to_string(),
                Style::default().fg(theme.border),
            )];
            spans.extend(styled_text_spans(segment.as_str(), quote_style, theme));
            Line::from(spans)
        })
        .collect()
}

fn render_markdown_list_item(
    marker: &str,
    text: &str,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let prefix = format!(" {marker} ");
    let available = width
        .saturating_sub(UnicodeWidthStr::width(prefix.as_str()))
        .max(8);
    let segments = wrap_visual_line(text, available);
    let indent = " ".repeat(UnicodeWidthStr::width(prefix.as_str()));
    let mut out = Vec::new();
    for (idx, segment) in segments.iter().enumerate() {
        let mut spans = Vec::new();
        if idx == 0 {
            spans.push(Span::styled(
                prefix.clone(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(indent.clone(), Style::default()));
        }
        spans.extend(styled_text_spans(
            segment,
            Style::default().fg(theme.foreground),
            theme,
        ));
        out.push(Line::from(spans));
    }
    out
}

fn render_markdown_paragraph(text: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let prefix = " ";
    let available = width.saturating_sub(UnicodeWidthStr::width(prefix)).max(8);
    wrap_visual_line(text, available)
        .into_iter()
        .map(|segment| {
            let mut spans = vec![Span::styled(prefix.to_string(), Style::default())];
            spans.extend(styled_text_spans(
                segment.as_str(),
                Style::default().fg(theme.foreground),
                theme,
            ));
            Line::from(spans)
        })
        .collect()
}

fn render_fenced_code_block(
    code_lang: &str,
    lines: &[String],
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let normalized = normalize_code_block_lines(lines, code_lang);
    let code_lang = canonical_code_lang(code_lang);
    let snapshot = highlight_code_block(code_lang.as_str(), normalized.as_slice());
    let label = if code_lang.is_empty() {
        " code block".to_string()
    } else {
        format!(" code block ({code_lang})")
    };
    let gutter_width = normalized.len().max(1).to_string().len();

    let mut out = Vec::new();
    out.push(popup_note_line(&label, theme.accent, theme));
    for (idx, line) in normalized.iter().enumerate() {
        let mapped = snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.per_line.get(idx))
            .and_then(|segments| map_line_segments_to_chars(line, segments).ok());
        out.extend(render_numbered_code_line(
            Some(idx + 1),
            line,
            code_lang.as_str(),
            mapped.as_deref(),
            theme,
            width,
            gutter_width,
        ));
    }
    if normalized.is_empty() {
        out.extend(render_numbered_code_line(
            None,
            "",
            code_lang.as_str(),
            None,
            theme,
            width,
            gutter_width,
        ));
    }
    out
}

fn normalize_code_block_lines(lines: &[String], code_lang: &str) -> Vec<String> {
    let code_text = lines.join("\n");
    if is_json_code_lang(code_lang)
        || (code_lang.is_empty()
            && matches!(code_text.trim().chars().next(), Some('{') | Some('[')))
    {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(code_text.trim()) {
            if let Ok(pretty) = serde_json::to_string_pretty(&value) {
                return pretty.lines().map(str::to_string).collect();
            }
        }
    }
    lines.to_vec()
}

fn render_math_block(lines: &[String], theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let prefix = " ∑ ";
    let continuation = "   ";
    let available = width.saturating_sub(UnicodeWidthStr::width(prefix)).max(8);
    let math_style = Style::default().fg(theme.accent).bg(dim_bg_towards(
        theme.cursor_line_bg,
        theme.background,
        40,
    ));

    let mut out = vec![popup_note_line(" equation", theme.accent, theme)];
    if lines.is_empty() {
        out.push(Line::from(vec![
            Span::styled(prefix.to_string(), math_style.add_modifier(Modifier::BOLD)),
            Span::styled(String::new(), math_style),
        ]));
        return out;
    }

    for (line_idx, line) in lines.iter().enumerate() {
        let wrapped = wrap_visual_line(line.trim(), available);
        for (segment_idx, segment) in wrapped.iter().enumerate() {
            let leading = if line_idx == 0 && segment_idx == 0 {
                prefix
            } else {
                continuation
            };
            let mut spans = vec![Span::styled(
                leading.to_string(),
                math_style.add_modifier(Modifier::BOLD),
            )];
            spans.extend(styled_math_spans(
                segment,
                math_style.add_modifier(Modifier::ITALIC),
                theme,
            ));
            out.push(Line::from(spans));
        }
    }
    out
}

fn parse_markdown_heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.chars().take_while(|ch| *ch == '#').count();
    if hashes == 0 {
        return None;
    }
    let text = line[hashes..].trim();
    (!text.is_empty()).then_some((hashes, text))
}

fn strong_only_heading_text(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with("**") || !trimmed.ends_with("**") || trimmed.len() < 4 {
        return None;
    }
    let inner = trimmed[2..trimmed.len() - 2].trim();
    if inner.is_empty() || inner.contains("**") {
        return None;
    }
    Some(inner.to_string())
}

fn parse_list_marker(line: &str) -> Option<(&str, &str)> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some((&marker[..1], rest.trim_start()));
        }
    }

    let digits = line.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digits > 0 && line[digits..].starts_with(". ") {
        let marker = &line[..digits + 1];
        let rest = line[digits + 2..].trim_start();
        if !rest.is_empty() {
            return Some((marker, rest));
        }
    }
    None
}

fn parse_code_fence(line: &str) -> Option<&str> {
    line.strip_prefix("```")
        .or_else(|| line.strip_prefix("~~~"))
        .map(str::trim)
}

fn parse_math_block_start(line: &str) -> Option<&'static str> {
    match line.trim() {
        "$$" => Some("$$"),
        "\\[" => Some("\\]"),
        _ => None,
    }
}

fn extract_single_line_math_block(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with("$$") && trimmed.ends_with("$$") && trimmed.len() > 4 {
        return Some(trimmed[2..trimmed.len() - 2].trim().to_string());
    }
    if trimmed.starts_with("\\[") && trimmed.ends_with("\\]") && trimmed.len() > 4 {
        return Some(trimmed[2..trimmed.len() - 2].trim().to_string());
    }
    None
}

fn is_thematic_rule(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= 3
        && (trimmed.chars().all(|ch| ch == '-')
            || trimmed.chars().all(|ch| ch == '*')
            || trimmed.chars().all(|ch| ch == '_'))
}
