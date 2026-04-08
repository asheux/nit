//! Integration tests for `nit_utils::time`.

use nit_utils::time::now_millis;

#[test]
fn now_millis_is_positive() {
    assert!(now_millis() > 0);
}

#[test]
fn now_millis_is_monotonic() {
    let a = now_millis();
    let b = now_millis();
    assert!(b >= a);
}
