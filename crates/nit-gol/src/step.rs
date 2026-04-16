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
            let neighbors = count_neighbors(grid, x, y, edge);
            let alive = grid.get(x, y);
            let survives = if alive {
                rule.is_survive(neighbors)
            } else {
                rule.is_birth(neighbors)
            };
            next.set(x, y, survives);
        }
    }
    next
}

fn count_neighbors(grid: &Grid, x: usize, y: usize, edge: EdgeMode) -> u8 {
    let width = grid.width() as isize;
    let height = grid.height() as isize;
    let mut count = 0u8;
    for dy in -1..=1isize {
        for dx in -1..=1isize {
            if dx == 0 && dy == 0 {
                continue;
            }
            if let Some((nx, ny)) = resolve_neighbor(x, y, dx, dy, width, height, edge) {
                count += u8::from(grid.get(nx, ny));
            }
        }
    }
    count
}

fn resolve_neighbor(
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
        EdgeMode::Dead => {
            if nx < 0 || ny < 0 || nx >= width || ny >= height {
                return None;
            }
            Some((nx as usize, ny as usize))
        }
        EdgeMode::Toroid => {
            let wx = ((nx % width) + width) % width;
            let wy = ((ny % height) + height) % height;
            Some((wx as usize, wy as usize))
        }
    }
}
