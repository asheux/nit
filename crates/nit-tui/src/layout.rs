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

// Column percentages sum to 100. The editor takes the widest slice because
// that is where typing happens; on the right, the visualiser is taller than
// the gate monitor for the same reason.
const COL_LEFT_PCT: u16 = 25;
const COL_EDITOR_PCT: u16 = 40;
const COL_RIGHT_PCT: u16 = 35;
const RIGHT_VISUALIZER_PCT: u16 = 55;
const RIGHT_GATE_PCT: u16 = 45;

const TOP_BAR_ROWS: u16 = 3;
const BOTTOM_BAR_ROWS: u16 = 2;
const CENTER_MIN_ROWS: u16 = 5;

pub fn split(frame: Rect) -> MainLayout {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(TOP_BAR_ROWS),
            Constraint::Min(CENTER_MIN_ROWS),
            Constraint::Length(BOTTOM_BAR_ROWS),
        ])
        .split(frame);
    let [top, center, bottom] = [rows[0], rows[1], rows[2]];

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(COL_LEFT_PCT),
            Constraint::Percentage(COL_EDITOR_PCT),
            Constraint::Percentage(COL_RIGHT_PCT),
        ])
        .split(center);

    let left_col = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(columns[0]);

    let right_col = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(RIGHT_VISUALIZER_PCT),
            Constraint::Percentage(RIGHT_GATE_PCT),
        ])
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
