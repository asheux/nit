use ratatui::layout::Rect;

/// Compute (cols, rows) for an N-pane grid using `cols = ceil(sqrt(N))`,
/// `rows = ceil(N / cols)`. Always at least 1×1.
pub fn compute_grid_shape(pane_count: usize) -> (usize, usize) {
    if pane_count == 0 {
        return (1, 1);
    }
    let cols = ((pane_count as f64).sqrt().ceil() as usize).max(1);
    let rows = pane_count.div_ceil(cols);
    (cols, rows)
}

/// Rect for the `pane_index`-th pane in a (cols × rows) grid laid out
/// across `area`. The integer-arithmetic partition function biases
/// remainders toward the rightmost / bottommost panes; row sums always
/// equal `area.height`, column sums always equal `area.width`.
pub fn pane_rect(area: Rect, cols: usize, rows: usize, pane_index: usize) -> Rect {
    if cols == 0 || rows == 0 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let column = (pane_index % cols) as u16;
    let row = (pane_index / cols) as u16;
    let cols_u16 = cols as u16;
    let rows_u16 = rows as u16;
    let x_start = cell_offset(area.width, cols_u16, column);
    let y_start = cell_offset(area.height, rows_u16, row);
    let x_end = cell_offset(area.width, cols_u16, column + 1);
    let y_end = cell_offset(area.height, rows_u16, row + 1);
    Rect::new(
        area.x + x_start,
        area.y + y_start,
        x_end - x_start,
        y_end - y_start,
    )
}

/// Hit-test a (column, row) point against the grid. Returns the pane
/// index, or `None` if the point lies outside `area`. Direct lookup —
/// does not iterate every pane.
pub fn pane_at_point(area: Rect, cols: usize, rows: usize, x: u16, y: u16) -> Option<usize> {
    if cols == 0 || rows == 0 {
        return None;
    }
    let local_x = x.checked_sub(area.x).filter(|dx| *dx < area.width)?;
    let local_y = y.checked_sub(area.y).filter(|dy| *dy < area.height)?;
    let column = locate_axis_index(area.width, cols as u16, local_x)? as usize;
    let row = locate_axis_index(area.height, rows as u16, local_y)? as usize;
    Some(row * cols + column)
}

fn cell_offset(extent: u16, divisions: u16, k: u16) -> u16 {
    if divisions == 0 {
        return 0;
    }
    ((extent as u32 * k as u32) / divisions as u32) as u16
}

fn locate_axis_index(extent: u16, divisions: u16, point: u16) -> Option<u16> {
    if divisions == 0 || point >= extent {
        return None;
    }
    let mut idx: u16 = 0;
    while idx + 1 < divisions && point >= cell_offset(extent, divisions, idx + 1) {
        idx += 1;
    }
    Some(idx)
}

#[cfg(test)]
mod tests {
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
}
