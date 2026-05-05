// Hilbert-curve index ↔ (x, y) mapping via iterative sub-quadrant traversal.
// Adapted from the canonical bit-interleaving routine (Butz / Moore). `order` is
// the log2 of the curve side length; `index` is the position along the curve.
pub(super) fn index_to_xy(order: u32, index: u32) -> (u32, u32) {
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

// Flip/rotate coordinates into the canonical quadrant orientation. Only the
// lower and upper-left sub-quadrants need a transform; the upper-right sub-quadrant
// passes through.
fn rotate_quadrant(n: u32, x: u32, y: u32, rx: u32, ry: u32) -> (u32, u32) {
    if ry == 0 {
        if rx == 1 {
            return (n - 1 - x, n - 1 - y);
        }
        return (y, x);
    }
    (x, y)
}

pub(super) fn order_for_width(width: usize) -> u32 {
    width.next_power_of_two().max(1).trailing_zeros()
}
