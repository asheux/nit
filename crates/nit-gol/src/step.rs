//! Single-generation grid evolution.
//!
//! Applies a [`Rule`] to every cell in a [`Grid`], producing the next
//! generation. Neighbor counting respects the chosen [`EdgeMode`].

use crate::{grid::EdgeMode, Grid, Rule};

/// Advance the grid by one generation under the given rule.
#[must_use]
pub fn step(grid: &Grid, rule: Rule, edge: EdgeMode) -> Grid {
    let width = grid.width();
    let height = grid.height();
    let mut next = Grid::new(width, height);
    if width == 0 || height == 0 {
        return next;
    }
    for y in 0..height {
        for x in 0..width {
            let neighbors = neighborhood::count(grid, x, y, edge);
            let alive = grid.get(x, y);
            let next_alive = if alive {
                rule.is_survive(neighbors)
            } else {
                rule.is_birth(neighbors)
            };
            next.set(x, y, next_alive);
        }
    }
    next
}

mod neighborhood {
    use crate::{grid::EdgeMode, Grid};

    // Fixed Moore neighborhood order keeps neighbor counting deterministic
    // across builds so regression tests comparing generation-by-generation
    // output stay stable.
    const MOORE_OFFSETS: [(isize, isize); 8] = [
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ];

    #[inline]
    pub(super) fn count(grid: &Grid, x: usize, y: usize, edge: EdgeMode) -> u8 {
        let width = grid.width() as isize;
        let height = grid.height() as isize;
        let mut total = 0u8;
        for (dx, dy) in MOORE_OFFSETS {
            if let Some((nx, ny)) = resolve(x, y, dx, dy, width, height, edge) {
                total += u8::from(grid.get(nx, ny));
            }
        }
        total
    }

    fn resolve(
        x: usize,
        y: usize,
        dx: isize,
        dy: isize,
        width: isize,
        height: isize,
        edge: EdgeMode,
    ) -> Option<(usize, usize)> {
        let nx = x as isize + dx;
        let ny = y as isize + dy;
        match edge {
            EdgeMode::Toroid => Some((
                nx.rem_euclid(width) as usize,
                ny.rem_euclid(height) as usize,
            )),
            EdgeMode::Dead => {
                if nx < 0 || ny < 0 || nx >= width || ny >= height {
                    None
                } else {
                    Some((nx as usize, ny as usize))
                }
            }
        }
    }
}
