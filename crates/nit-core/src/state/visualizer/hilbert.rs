//! Hilbert space-filling curve helpers used by the visualizer's inspector.
//!
//! The `Structural` and `HilbertBits` encoders walk a Hilbert curve over the
//! seed grid so neighbouring cells in linear order stay close in 2-D — this
//! makes the inspector's "jump to byte index" gesture meaningful in those
//! views. The math here is the canonical iterative variant from
//! "Hacker's Delight" (and matches `wikipedia:Hilbert_curve#Applications`).

/// Smallest power-of-two order that covers a square grid of `size` cells per
/// side. The Hilbert curve is defined on `2^order × 2^order` lattices, so the
/// caller pads to the next power of two.
pub(super) fn hilbert_order_for(size: usize) -> u32 {
    let mut order = 0u32;
    let mut n = 1usize;
    while n < size {
        n <<= 1;
        order += 1;
    }
    order
}

/// Map a linear `index` along the Hilbert curve of side `2^order` to its
/// `(x, y)` coordinates on the grid.
pub(super) fn hilbert_index_to_xy(order: u32, index: u32) -> (u32, u32) {
    let mut x = 0u32;
    let mut y = 0u32;
    let mut t = index;
    let mut s = 1u32;
    let n = 1u32 << order;
    while s < n {
        let rx = (t / 2) & 1;
        let ry = (t ^ rx) & 1;
        let (nx, ny) = rotate_quadrant(s, x, y, rx, ry);
        x = nx + s * rx;
        y = ny + s * ry;
        t /= 4;
        s *= 2;
    }
    (x, y)
}

/// Reflect / rotate the lower-left quadrant of an `n × n` Hilbert tile so
/// the recursion stays consistent across nesting levels. Inlined from the
/// canonical implementation; the four `(rx, ry)` cases pick which symmetry
/// to apply.
fn rotate_quadrant(n: u32, x: u32, y: u32, rx: u32, ry: u32) -> (u32, u32) {
    if ry == 0 {
        if rx == 1 {
            return (n - 1 - x, n - 1 - y);
        }
        return (y, x);
    }
    (x, y)
}
