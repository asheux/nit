use super::*;
use std::collections::HashSet;

#[test]
fn compute_grid_shape_table() {
    assert_eq!(compute_grid_shape(1), (1, 1));
    assert_eq!(compute_grid_shape(2), (2, 1));
    assert_eq!(compute_grid_shape(4), (2, 2));
    assert_eq!(compute_grid_shape(6), (3, 2));
    assert_eq!(compute_grid_shape(8), (3, 3));
    assert_eq!(compute_grid_shape(16), (4, 4));
    assert_eq!(compute_grid_shape(32), (6, 6));
}

fn cells_in_rect(rect: Rect) -> impl Iterator<Item = (u16, u16)> {
    let xs = rect.x..rect.x.saturating_add(rect.width);
    let ys = rect.y..rect.y.saturating_add(rect.height);
    ys.flat_map(move |y| xs.clone().map(move |x| (x, y)))
}

#[test]
fn pane_rect_partitions_area_without_overlap() {
    let area = Rect::new(0, 0, 80, 30);
    for n in [1usize, 2, 4, 6, 8, 16, 32] {
        let (cols, rows) = compute_grid_shape(n);
        let cells: HashSet<(u16, u16)> = (0..n)
            .flat_map(|idx| cells_in_rect(pane_rect(area, cols, rows, idx)))
            .collect();
        let pane_cell_total: usize = (0..n)
            .map(|idx| {
                let r = pane_rect(area, cols, rows, idx);
                (r.width as usize) * (r.height as usize)
            })
            .sum();
        assert_eq!(cells.len(), pane_cell_total, "overlap detected for n={n}");
        if cols * rows == n {
            let total = (area.width as usize) * (area.height as usize);
            assert_eq!(cells.len(), total, "uncovered cells for n={n}");
        }
    }
}

#[test]
fn pane_at_point_round_trips() {
    let area = Rect::new(0, 0, 80, 30);
    for n in [1usize, 2, 4, 8, 16] {
        let (cols, rows) = compute_grid_shape(n);
        for idx in 0..n {
            let r = pane_rect(area, cols, rows, idx);
            if r.width == 0 || r.height == 0 {
                continue;
            }
            let mid_x = r.x + r.width / 2;
            let mid_y = r.y + r.height / 2;
            let hit = pane_at_point(area, cols, rows, mid_x, mid_y);
            assert_eq!(hit, Some(idx), "n={n} idx={idx} mid=({mid_x},{mid_y})");
        }
    }
}

#[test]
fn pane_at_point_outside_area_returns_none() {
    let area = Rect::new(10, 5, 40, 20);
    let (cols, rows) = compute_grid_shape(4);
    assert_eq!(pane_at_point(area, cols, rows, 0, 0), None);
    assert_eq!(pane_at_point(area, cols, rows, 100, 100), None);
    assert_eq!(pane_at_point(area, cols, rows, 9, 5), None);
    assert_eq!(pane_at_point(area, cols, rows, 50, 5), None);
}
