//! The normal station shares its center pane between the conversation, source and shell.
use crate::theme::Theme;
use nit_core::StationCenterTab;
use ratatui::{
    layout::{Position, Rect},
    style::{Modifier, Style},
    widgets::{Block, Paragraph},
    Frame,
};

fn labels(width: u16) -> [&'static str; 2] {
    if width >= 21 {
        [" Agent Chat ", " Editor "]
    } else if width >= 10 {
        ["Chat", "Editor"]
    } else {
        ["C", "E"]
    }
}

fn tabs(area: Rect) -> impl Iterator<Item = (StationCenterTab, Rect, &'static str)> {
    let mut x = area.x;
    [StationCenterTab::Chat, StationCenterTab::Editor]
        .into_iter()
        .zip(labels(area.width))
        .filter_map(move |(tab, label)| {
            let width = label.len() as u16;
            let r = Rect::new(x, area.y, width, 1);
            x = x.saturating_add(width + u16::from(area.width >= 20));
            (area.height > 0 && r.right() <= area.right()).then_some((tab, r, label))
        })
}

pub(crate) fn tab_at(area: Rect, point: Position) -> Option<StationCenterTab> {
    tabs(area)
        .find(|(_, r, _)| r.contains(point))
        .map(|(tab, _, _)| tab)
}

pub(crate) fn render(
    frame: &mut Frame,
    area: Rect,
    active: StationCenterTab,
    switch_shortcut: bool,
    theme: &Theme,
) {
    if area.is_empty() {
        return;
    }
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.list_background())),
        area,
    );
    let mut right = area.x;
    for (tab, r, label) in tabs(area) {
        let style = if tab == active
            || (tab == StationCenterTab::Chat && active == StationCenterTab::Terminal)
        {
            Style::default()
                .fg(theme.title_focused)
                .bg(theme.selection_bg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(theme.muted)
        };
        frame.render_widget(Paragraph::new(label).style(style), r);
        right = r.right();
    }
    let hint = "F6 switch";
    if switch_shortcut && area.right().saturating_sub(right) >= hint.len() as u16 + 2 {
        frame.render_widget(
            Paragraph::new(hint).style(Style::default().fg(theme.muted)),
            Rect::new(
                area.right() - hint.len() as u16 - 1,
                area.y,
                hint.len() as u16,
                1,
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn pointer_targets_match_the_drawn_tabs_at_each_width() {
        let theme = Theme::default();
        for width in [3, 11, 20, 32, 33, 62] {
            let area = Rect::new(1, 1, width, 1);
            let mut t = Terminal::new(TestBackend::new(width + 2, 3)).unwrap();
            t.draw(|f| render(f, area, StationCenterTab::Chat, true, &theme))
                .unwrap();
            let buffer = t.backend().buffer();
            let spans = tabs(area).collect::<Vec<_>>();
            assert_eq!(spans.len(), 2);
            for (tab, r, label) in spans {
                assert_eq!(tab_at(area, (r.x, r.y).into()), Some(tab));
                assert_eq!(tab_at(area, (r.right() - 1, r.y).into()), Some(tab));
                assert_eq!(
                    (r.x..r.right())
                        .map(|x| buffer.get(x, r.y).symbol())
                        .collect::<String>(),
                    label
                );
                assert_eq!(
                    buffer.get(r.x, r.y).bg,
                    if tab == StationCenterTab::Chat {
                        theme.selection_bg
                    } else {
                        theme.list_background()
                    }
                );
            }
            assert_eq!(tab_at(area, (area.x, area.y + 1).into()), None);
        }
    }
}
