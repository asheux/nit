use nit_core::{AppState, FileTreeKind, FileTreeRow, PaneId};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::theme::Theme;

const INDENT_STEP: usize = 2;
const ARROW_CHARS: usize = 2;
const FILE_FG_BLEND: f32 = 0.28;
const BLOCK_BORDER_ROWS: u16 = 2;

/// Paint the file tree pane into `area`, highlighting the selected row.
pub fn render(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let focused = state.focus == PaneId::Editor;
    let commands = header_commands(state);
    let block = build_block(theme, focused, &commands);

    let inner_height = area.height.saturating_sub(BLOCK_BORDER_ROWS) as usize;
    let rows = &state.file_tree.rows;
    let scroll = state.file_tree.scroll_offset;
    let selected = state.file_tree.selected;
    let file_fg = blend(theme.foreground, theme.title, FILE_FG_BLEND);

    let end = (scroll + inner_height).min(rows.len());
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .take(end)
        .skip(scroll)
        .map(|(idx, row)| row_line(row, idx == selected, theme, file_fg))
        .collect();

    let paragraph = Paragraph::new(lines)
        .block(block)
        .style(Style::default().bg(theme.background).fg(theme.foreground));
    f.render_widget(paragraph, area);
}

fn header_commands(state: &AppState) -> String {
    let hidden = on_off(state.file_tree.show_hidden);
    let ignored = on_off(state.file_tree.show_ignored);
    format!(
        "[Enter open/toggle] [Esc/q close] [r refresh] [. hidden:{hidden}] [i ignored:{ignored}]"
    )
}

fn on_off(flag: bool) -> &'static str {
    if flag {
        "ON"
    } else {
        "OFF"
    }
}

fn build_block<'a>(theme: &Theme, focused: bool, commands: &'a str) -> Block<'a> {
    let (border_color, border_type, title_color) = if focused {
        (theme.border_focused, BorderType::Thick, theme.title_focused)
    } else {
        (theme.border, BorderType::Plain, theme.title)
    };
    let title = Line::from(vec![
        Span::styled(
            "NITTREE",
            Style::default()
                .fg(title_color)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            commands,
            Style::default()
                .fg(theme.border)
                .bg(theme.background)
                .add_modifier(Modifier::DIM),
        ),
    ]);
    Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.background))
        .border_style(Style::default().fg(border_color))
        .border_type(border_type)
        .title(title)
}

fn row_line<'a>(row: &'a FileTreeRow, selected: bool, theme: &Theme, file_fg: Color) -> Line<'a> {
    let mut style = match row.kind {
        FileTreeKind::Dir => Style::default().fg(theme.title),
        FileTreeKind::File => Style::default().fg(file_fg),
        FileTreeKind::Loading => Style::default().fg(theme.warning),
    };
    if selected {
        style = style.bg(theme.selection_bg).add_modifier(Modifier::BOLD);
    }

    if matches!(row.kind, FileTreeKind::Dir) {
        return render_dir_row(row, style, theme);
    }
    Line::from(Span::styled(row.text.clone(), style))
}

fn render_dir_row<'a>(row: &'a FileTreeRow, style: Style, theme: &Theme) -> Line<'a> {
    let indent_chars = row.depth.saturating_mul(INDENT_STEP);
    let mut chars = row.text.chars();
    let indent: String = chars.by_ref().take(indent_chars).collect();
    let arrow: String = chars.by_ref().take(ARROW_CHARS).collect();
    let rest: String = chars.collect();
    if arrow.is_empty() {
        return Line::from(Span::styled(row.text.clone(), style));
    }
    let arrow_style = style.fg(theme.accent).add_modifier(Modifier::BOLD);
    Line::from(vec![
        Span::styled(indent, style),
        Span::styled(arrow, arrow_style),
        Span::styled(rest, style),
    ])
}

fn blend(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            let mix = |x: u8, y: u8| -> u8 {
                let xf = x as f32;
                let yf = y as f32;
                (xf * (1.0 - t) + yf * t).round().clamp(0.0, 255.0) as u8
            };
            Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
        }
        _ => a,
    }
}
