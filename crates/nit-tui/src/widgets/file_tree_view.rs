use nit_core::state::file_tree::{FileTreePrompt, FileTreePromptKind};
use nit_core::{AppState, FileTreeKind, FileTreeRow, PaneId};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::theme::Theme;

const INDENT_STEP: usize = 2;
const ARROW_CHARS: usize = 2;

pub fn render(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let focused = state.focus == PaneId::Editor;
    let commands = header_commands(state);
    let block = build_block(theme, focused, &commands);

    let inner = block.inner(area);
    f.render_widget(block, area);
    let tree = &state.file_tree;
    for (offset, row) in tree
        .rows
        .iter()
        .skip(tree.scroll_offset)
        .take(inner.height as usize)
        .enumerate()
    {
        let selected = tree.scroll_offset + offset == tree.selected;
        let bg = if selected && focused {
            theme.selection_bg
        } else if selected {
            theme.cursor_line_bg
        } else {
            theme.background
        };
        let mut line = row_line(row, theme);
        let marker = if selected && focused { "▸ " } else { "  " };
        line.spans.insert(
            0,
            Span::styled(marker, Style::default().fg(theme.border_focused)),
        );
        let mut style = Style::default().bg(bg);
        if selected && focused {
            style = style.add_modifier(Modifier::BOLD);
        }
        f.render_widget(
            Paragraph::new(line).style(style),
            Rect::new(inner.x, inner.y + offset as u16, inner.width, 1),
        );
    }

    if let Some(prompt) = &state.file_tree.prompt {
        render_prompt_bar(f, area, prompt, theme);
    }
}

fn render_prompt_bar(f: &mut Frame, area: Rect, prompt: &FileTreePrompt, theme: &Theme) {
    if area.height < 3 || area.width < 4 {
        return;
    }
    let label = match prompt.kind {
        FileTreePromptKind::Rename => "rename",
        FileTreePromptKind::NewFile => "new file",
        FileTreePromptKind::NewDir => "new dir",
    };
    let bar = Rect {
        x: area.x + 1,
        y: area.y + area.height - 2,
        width: area.width.saturating_sub(2),
        height: 1,
    };
    let text = format!(" {label} ▸ {}\u{2588}", prompt.input);
    let style = Style::default()
        .fg(theme.background)
        .bg(theme.accent)
        .add_modifier(Modifier::BOLD);
    f.render_widget(Paragraph::new(Line::from(text)).style(style), bar);
}

fn header_commands(state: &AppState) -> String {
    let hidden = on_off(state.file_tree.show_hidden);
    let ignored = on_off(state.file_tree.show_ignored);
    format!(
        "[Enter open] [r rename] [n new] [N dir] [R refresh] [. hidden:{hidden}] [i ignored:{ignored}] [Esc close]"
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
        Span::styled(commands, theme.status_idle_style().bg(theme.background)),
    ]);
    Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.background))
        .border_style(Style::default().fg(border_color))
        .border_type(border_type)
        .title(title)
}

fn row_line<'a>(row: &'a FileTreeRow, theme: &Theme) -> Line<'a> {
    let style = match row.kind {
        FileTreeKind::Dir => Style::default().fg(theme.title),
        FileTreeKind::File => Style::default().fg(theme.foreground),
        FileTreeKind::Loading => theme.status_idle_style(),
    };
    if row.kind == FileTreeKind::Dir {
        return render_dir_row(row, style, theme);
    }
    Line::from(Span::styled(row.text.as_str(), style))
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
    let arrow_style = style.fg(theme.muted);
    Line::from(vec![
        Span::styled(indent, style),
        Span::styled(arrow, arrow_style),
        Span::styled(rest, style),
    ])
}

#[cfg(test)]
#[path = "../tests/file_tree_view.rs"]
mod tests;
