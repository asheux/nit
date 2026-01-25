use nit_core::AppState;
use ratatui::{
    layout::Alignment,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::Theme;

pub fn render(frame: &mut Frame, area: ratatui::layout::Rect, state: &AppState, theme: &Theme) {
    let (line, col) = state.line_col();
    let mode = format!("{:?}", state.mode).to_uppercase();
    let file = state
        .editor_buffer()
        .path()
        .map(|p| p.display().to_string());
    let dirty = if state.editor_buffer().is_dirty() {
        "*"
    } else {
        ""
    };
    let file_text = file.unwrap_or_else(|| state.editor_buffer().name().to_string());
    let status_label = state
        .status
        .as_ref()
        .map(|s| format!("STATUS: {s}"))
        .unwrap_or_default();

    let mut hint_labels = vec![
        "Ctrl+Q Quit",
        "Ctrl+S Save",
        "Tab Focus",
        "Ctrl+HJKL Pane",
        "F1/? Help",
        "Ctrl+L Clear",
        "Ctrl+Shift+S Syntax",
        "Ctrl+B Debug",
        "Ctrl+G Search",
        "Ctrl+T Wrap",
        "Ctrl+R Seed",
        "Ctrl+A Apply",
        "Ctrl+N Snap",
        "Space Pause",
        "+/- Speed",
    ];

    let inner_width = area.width.saturating_sub(2) as usize;
    let fixed_width = [
        " nit ",
        " | ",
        " | ",
        &mode,
        " | UTF-8 | ",
        &format!("Ln {}, Col {}", line, col),
    ]
    .iter()
    .map(|s| s.width())
    .sum::<usize>();

    let mut hints_text = hints_join(&hint_labels);
    let mut hints_width = hints_text.width();
    let mut status_width = status_label.width();
    let mut right_width = hints_width
        + if hints_width > 0 && status_width > 0 { 2 } else { 0 }
        + status_width;
    let mut max_right = inner_width.saturating_sub(fixed_width);
    if right_width > 0 {
        max_right = max_right.saturating_sub(1);
    }

    while right_width > max_right && !hint_labels.is_empty() {
        hint_labels.pop();
        hints_text = hints_join(&hint_labels);
        hints_width = hints_text.width();
        right_width = hints_width
            + if hints_width > 0 && status_width > 0 { 2 } else { 0 }
            + status_width;
    }

    let mut status_label = status_label;
    if right_width > max_right {
        status_label = truncate_start(&status_label, max_right);
        status_width = status_label.width();
        right_width = status_width;
        hints_text.clear();
        hints_width = 0;
    }

    let right_gap = if right_width > 0 { 1 } else { 0 };
    let left_max = inner_width.saturating_sub(right_width + right_gap);
    let file_max = left_max.saturating_sub(fixed_width);

    let file_display = if dirty.is_empty() {
        truncate_start(&file_text, file_max)
    } else {
        let star_width = "*".width();
        let name_max = file_max.saturating_sub(star_width);
        format!("{}*", truncate_start(&file_text, name_max))
    };

    let mut spans = vec![
        Span::styled(
            " nit ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | ", Style::default().fg(theme.border)),
        Span::styled(
            file_display,
            Style::default()
                .fg(if dirty.is_empty() {
                    theme.foreground
                } else {
                    theme.warning
                })
                .add_modifier(if dirty.is_empty() {
                    Modifier::empty()
                } else {
                    Modifier::BOLD
                }),
        ),
        Span::styled(" | ", Style::default().fg(theme.border)),
        Span::styled(mode, Style::default().fg(theme.accent)),
        Span::styled(" | UTF-8 | ", Style::default().fg(theme.border)),
        Span::styled(
            format!("Ln {}, Col {}", line, col),
            Style::default().fg(theme.foreground),
        ),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " NEURAL INTERFACE TERMINAL ",
            Style::default()
                .fg(theme.title)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.background));

    if right_width > 0 {
        let left_width: usize = spans
            .iter()
            .map(|s| s.content.as_ref().width())
            .sum();
        let pad = inner_width
            .saturating_sub(left_width)
            .saturating_sub(right_width);
        let gap = if pad == 0 { 1 } else { pad };
        spans.push(Span::raw(" ".repeat(gap)));

        if !hints_text.is_empty() {
            spans.push(Span::styled(
                hints_text,
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM),
            ));
        }

        if !status_label.is_empty() {
            if hints_width > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                status_label,
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM),
            ));
        }
    }

    let line = Line::from(spans);
    let para = Paragraph::new(line)
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .alignment(Alignment::Left)
        .block(block);

    frame.render_widget(para, area);
}

fn hints_join(labels: &[&str]) -> String {
    labels
        .iter()
        .map(|label| format!("[ {label} ]"))
        .collect::<Vec<_>>()
        .join("  ")
}

fn truncate_start(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let mut width = 0;
    let mut idx = text.len();
    for (i, ch) in text.char_indices().rev() {
        width += UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        if width >= max_width.saturating_sub(1) {
            idx = i;
            break;
        }
    }
    format!("…{}", &text[idx..])
}
