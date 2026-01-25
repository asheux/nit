use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub struct MainLayout {
    pub top: Rect,
    pub bottom: Rect,
    pub notes: Rect,
    pub job: Rect,
    pub editor: Rect,
    pub visualizer: Rect,
    pub gate: Rect,
}

pub fn split(frame: Rect) -> MainLayout {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(frame);

    let top = vertical[0];
    let center = vertical[1];
    let bottom = vertical[2];

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(40),
            Constraint::Percentage(35),
        ])
        .split(center);

    let left_col = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(columns[0]);

    let right_col = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(columns[2]);

    MainLayout {
        top,
        bottom,
        notes: left_col[0],
        job: left_col[1],
        editor: columns[1],
        visualizer: right_col[0],
        gate: right_col[1],
    }
}
