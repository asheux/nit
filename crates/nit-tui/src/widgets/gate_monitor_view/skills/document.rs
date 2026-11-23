//! Formats the blocks used in the bundled playbooks. The catalog supplies the title.
use crate::theme::Theme;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) fn lines(body: &str, width: usize, theme: &Theme, heading: Color) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let title = Style::default().fg(heading).add_modifier(Modifier::BOLD);
    let normal = Style::default().fg(theme.foreground);
    let mut source = body.trim().lines().map(str::trim).peekable();
    if source.peek().is_some_and(|line| line.starts_with("# ")) {
        source.next();
    }
    while let Some(line) = source.next() {
        if line.is_empty() {
            if !out.is_empty() && out.last().is_some_and(|l: &Line| l.width() > 0) {
                out.push(Line::default());
            }
        } else if line.starts_with('|') {
            let mut rows = vec![line];
            while source.peek().is_some_and(|line| line.starts_with('|')) {
                rows.push(source.next().unwrap());
            }
            table(&rows, width, theme, title, &mut out);
        } else if let Some(heading) = line
            .trim_start_matches('#')
            .strip_prefix(' ')
            .filter(|_| line.starts_with('#'))
        {
            out.extend(wrap(heading, width, title, theme));
        } else if let Some((label, text)) = line.split_once(':').filter(|(label, _)| {
            label.len() <= 70
                && label.chars().any(|c| c.is_ascii_uppercase())
                && !label.chars().any(|c| c.is_ascii_lowercase())
        }) {
            out.extend(wrap(label, width, title, theme));
            out.extend(wrap(text, width, normal, theme));
        } else if line.ends_with(':') {
            out.extend(wrap(line.trim_end_matches(':'), width, title, theme));
        } else {
            let list = line.strip_prefix("- ").map(|body| ("•", body)).or_else(|| {
                let (n, body) = line.split_once(". ")?;
                n.parse::<usize>()
                    .ok()
                    .map(|_| (&line[..n.len() + 1], body))
            });
            if let Some((prefix, text)) = list {
                let indent = (prefix.chars().count() + 1).min(width.saturating_sub(1));
                for (i, mut row) in wrap(text, width - indent, normal, theme)
                    .into_iter()
                    .enumerate()
                {
                    let prefix = if i == 0 && indent >= prefix.chars().count() {
                        format!("{prefix:<indent$}")
                    } else {
                        " ".repeat(indent)
                    };
                    row.spans.insert(0, Span::styled(prefix, title));
                    out.push(row);
                }
            } else {
                out.extend(wrap(line, width, normal, theme));
            }
        }
    }
    out
}

fn table(rows: &[&str], width: usize, theme: &Theme, key: Style, out: &mut Vec<Line<'static>>) {
    let cells: Vec<Vec<_>> = rows
        .iter()
        .map(|row| {
            row.trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect::<Vec<_>>()
        })
        .filter(|r| !r.iter().all(|s| s.chars().all(|c| matches!(c, '-' | ':'))))
        .collect();
    let Some(headers) = cells.first() else { return };
    let normal = Style::default().fg(theme.foreground);
    let widths: Vec<_> = (0..headers.len())
        .map(|i| {
            cells
                .iter()
                .filter_map(|r| r.get(i))
                .map(|s| s.trim_matches('`').width())
                .max()
                .unwrap_or(0)
        })
        .collect();
    if widths.iter().sum::<usize>() + 2 * headers.len().saturating_sub(1) <= width {
        for (i, row) in cells.iter().enumerate() {
            let mut spans = Vec::new();
            for (j, cell) in row.iter().enumerate() {
                let style = if i == 0 { key } else { normal };
                if let Some(line) = wrap(cell, width, style, theme).pop() {
                    let used = line.width();
                    spans.extend(line.spans);
                    if j + 1 < row.len() {
                        spans.push(Span::raw(" ".repeat(widths[j].saturating_sub(used) + 2)));
                    }
                }
            }
            out.push(Line::from(spans));
        }
    } else {
        for (i, row) in cells.iter().skip(1).enumerate() {
            if i > 0 {
                out.push(Line::default());
            }
            for (j, (label, cell)) in headers.iter().zip(row).enumerate() {
                let value = format!("{label} · {cell}");
                out.extend(wrap(
                    &value,
                    width,
                    if j == 0 { key } else { normal },
                    theme,
                ));
            }
        }
    }
}

// Keep inline styles across word wraps and break long paths on terminal cell boundaries.
fn wrap(text: &str, width: usize, base: Style, theme: &Theme) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    let mut spans = Vec::new();
    let mut used = 0;
    let mut code = false;
    let mut bold = false;
    for word in text.split_whitespace() {
        let mut glyphs = Vec::new();
        let mut chars = word.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '`' {
                code = !code;
                continue;
            }
            if !code && ch == '*' && chars.next_if_eq(&'*').is_some() {
                bold = !bold;
                continue;
            }
            let mut style = base;
            if code {
                style = style.fg(theme.hl.string);
            }
            if bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            glyphs.push((ch, style));
        }
        let word_width = glyphs
            .iter()
            .map(|(ch, _)| ch.width().unwrap_or(0))
            .sum::<usize>();
        if used > 0 && used + 1 + word_width > width {
            rows.push(Line::from(std::mem::take(&mut spans)));
            used = 0;
        }
        if used > 0 {
            push(&mut spans, ' ', base);
            used += 1;
        }
        for (ch, style) in glyphs {
            let size = ch.width().unwrap_or(0);
            if used > 0 && used + size > width {
                rows.push(Line::from(std::mem::take(&mut spans)));
                used = 0;
            }
            push(&mut spans, ch, style);
            used += size;
        }
    }
    if !spans.is_empty() {
        rows.push(Line::from(spans));
    }
    rows
}

fn push(spans: &mut Vec<Span<'static>>, ch: char, style: Style) {
    if let Some(last) = spans.last_mut().filter(|span| span.style == style) {
        last.content.to_mut().push(ch);
    } else {
        spans.push(Span::styled(ch.to_string(), style));
    }
}

#[cfg(test)]
#[path = "../../../tests/skills_document.rs"]
mod tests;
