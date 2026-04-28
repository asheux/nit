use nit_core::MultipaneState;
use ratatui::layout::Rect;

use super::grid::pane_at_point;

pub fn cycle_forward(mp: &mut MultipaneState) {
    if mp.panes.is_empty() {
        return;
    }
    mp.focused = (mp.focused + 1) % mp.panes.len();
}

pub fn cycle_backward(mp: &mut MultipaneState) {
    if mp.panes.is_empty() {
        return;
    }
    if mp.focused == 0 {
        mp.focused = mp.panes.len() - 1;
    } else {
        mp.focused -= 1;
    }
}

pub fn focus_at_point(mp: &mut MultipaneState, area: Rect, x: u16, y: u16) -> Option<usize> {
    let idx = pane_at_point(area, mp.grid_cols, mp.grid_rows, x, y)?;
    if idx >= mp.panes.len() {
        return None;
    }
    mp.focused = idx;
    Some(idx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nit_core::PaneSession;

    fn build_mp(n: usize) -> MultipaneState {
        let panes = (0..n)
            .map(|i| PaneSession {
                pane_id: i,
                ..PaneSession::default()
            })
            .collect();
        let (cols, rows) = super::super::grid::compute_grid_shape(n);
        MultipaneState {
            backend_agent_id: "test".into(),
            panes,
            focused: 0,
            grid_cols: cols,
            grid_rows: rows,
            backend_filter: Some("test".into()),
        }
    }

    #[test]
    fn cycle_focus_wraps_forward_and_backward() {
        let mut mp = build_mp(4);
        cycle_forward(&mut mp);
        assert_eq!(mp.focused, 1);
        cycle_forward(&mut mp);
        assert_eq!(mp.focused, 2);
        cycle_forward(&mut mp);
        assert_eq!(mp.focused, 3);
        cycle_forward(&mut mp);
        assert_eq!(mp.focused, 0);

        cycle_backward(&mut mp);
        assert_eq!(mp.focused, 3);
        cycle_backward(&mut mp);
        assert_eq!(mp.focused, 2);
    }

    #[test]
    fn cycle_focus_no_op_when_empty() {
        let mut mp = build_mp(0);
        cycle_forward(&mut mp);
        assert_eq!(mp.focused, 0);
        cycle_backward(&mut mp);
        assert_eq!(mp.focused, 0);
    }

    #[test]
    fn focus_at_point_updates_focused() {
        let mut mp = build_mp(4);
        let area = Rect::new(0, 0, 80, 30);
        // pane 1 is top-right in a 2x2 grid
        let r = super::super::grid::pane_rect(area, 2, 2, 1);
        let mid_x = r.x + r.width / 2;
        let mid_y = r.y + r.height / 2;
        assert_eq!(focus_at_point(&mut mp, area, mid_x, mid_y), Some(1));
        assert_eq!(mp.focused, 1);
    }

    #[test]
    fn focus_at_point_outside_no_op() {
        let mut mp = build_mp(4);
        mp.focused = 2;
        let area = Rect::new(10, 10, 40, 20);
        assert_eq!(focus_at_point(&mut mp, area, 0, 0), None);
        assert_eq!(mp.focused, 2);
    }
}
