use crate::{grid::EdgeMode, Grid, Rule};

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

fn count_neighbors(grid: &Grid, x: usize, y: usize, edge: EdgeMode) -> u8 {
    let width = grid.width() as isize;
    let height = grid.height() as isize;
    let mut count = 0u8;
    for dy in -1..=1 {
        for dx in -1..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let nx = x as isize + dx;
            let ny = y as isize + dy;
            let (nx, ny) = match edge {
                EdgeMode::Dead => {
                    if nx < 0 || ny < 0 || nx >= width || ny >= height {
                        continue;
                    }
                    (nx, ny)
                }
                EdgeMode::Toroid => {
                    let wrapped_x = (nx + width) % width;
                    let wrapped_y = (ny + height) % height;
                    (wrapped_x, wrapped_y)
                }
            };
            if grid.get(nx as usize, ny as usize) {
                count += 1;
            }
        }
    }
    count
}
