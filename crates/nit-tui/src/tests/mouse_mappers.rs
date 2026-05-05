use super::drag_auto_scroll;

#[test]
fn no_change_when_inside_area() {
    let mut scroll = 5;
    // area: y=10, height=20 → bottom = 30. Cursor at 15 is inside.
    assert!(!drag_auto_scroll(15, 10, 20, &mut scroll, 100));
    assert_eq!(scroll, 5);
}

#[test]
fn scrolls_up_when_above_top() {
    let mut scroll = 50;
    // Cursor at row 5, area starts at 10 → 5 rows above. Scroll drops by 5.
    assert!(drag_auto_scroll(5, 10, 20, &mut scroll, 100));
    assert_eq!(scroll, 45);
}

#[test]
fn scrolls_down_when_below_bottom() {
    let mut scroll = 50;
    // area: y=10, height=20 → bottom row index 29 (inclusive).
    // Cursor at 30 is one past the bottom → scroll bumps by 1 so even
    // a single-row overshoot still scrolls one row.
    assert!(drag_auto_scroll(30, 10, 20, &mut scroll, 100));
    assert_eq!(scroll, 51);
}

#[test]
fn scales_with_overshoot_distance() {
    // Operator slamming the cursor 50 rows past the bottom shouldn't have
    // to wiggle to scroll each row.
    let mut scroll = 0;
    // Bottom = 30. Cursor at 80 → 51 rows past.
    assert!(drag_auto_scroll(80, 10, 20, &mut scroll, 100));
    assert_eq!(scroll, 51);
}

#[test]
fn no_change_when_already_at_top() {
    let mut scroll = 0;
    assert!(!drag_auto_scroll(0, 10, 20, &mut scroll, 100));
    assert_eq!(scroll, 0);
}

#[test]
fn clamps_to_max_scroll() {
    let mut scroll = 99;
    assert!(drag_auto_scroll(40, 10, 20, &mut scroll, 100));
    assert_eq!(scroll, 100);
    // Subsequent drag past bottom does not move further.
    assert!(!drag_auto_scroll(40, 10, 20, &mut scroll, 100));
    assert_eq!(scroll, 100);
}

// Zero-height area is the divisor-zero analog for the bottom calculation;
// guard against it so the mapper stays safe under odd terminal sizes.
#[test]
fn no_change_when_area_height_zero() {
    let mut scroll = 5;
    assert!(!drag_auto_scroll(0, 10, 0, &mut scroll, 100));
    assert_eq!(scroll, 5);
    assert!(!drag_auto_scroll(50, 10, 0, &mut scroll, 100));
    assert_eq!(scroll, 5);
}
