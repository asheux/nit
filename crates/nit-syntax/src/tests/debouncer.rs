use std::thread;
use std::time::Duration;

use crate::{Debouncer, DebouncerPhase};

#[test]
fn debouncer_idle_to_ready_cycle() {
    let mut d = Debouncer::new(0);
    assert_eq!(d.phase(), DebouncerPhase::Idle);
    assert!(!d.ready());

    d.mark();
    assert_eq!(d.phase(), DebouncerPhase::Ready);
    assert!(d.ready());

    d.clear();
    assert_eq!(d.phase(), DebouncerPhase::Idle);
}

#[test]
fn debouncer_pending_with_long_quiet_period() {
    let mut d = Debouncer::new(60_000);
    assert_eq!(d.phase(), DebouncerPhase::Idle);
    d.mark();
    assert_eq!(d.phase(), DebouncerPhase::Pending);
    assert!(!d.ready());
}

#[test]
fn debouncer_remark_resets_quiet_period_clock() {
    // After becoming Ready, a fresh `mark()` must reset the quiet-period
    // clock — the debouncer should swing back to Pending until a new
    // quiet period elapses, not stay Ready by accident. Sleep margin is
    // 4× the quiet period to absorb scheduler jitter on slow CI.
    let mut d = Debouncer::new(20);
    d.mark();
    thread::sleep(Duration::from_millis(80));
    assert_eq!(d.phase(), DebouncerPhase::Ready);

    d.mark();
    assert_eq!(d.phase(), DebouncerPhase::Pending);
}

#[test]
fn debouncer_clear_from_pending_returns_to_idle() {
    let mut d = Debouncer::new(60_000);
    d.mark();
    assert_eq!(d.phase(), DebouncerPhase::Pending);

    d.clear();
    assert_eq!(d.phase(), DebouncerPhase::Idle);
}
