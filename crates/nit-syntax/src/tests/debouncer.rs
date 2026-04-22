use crate::{Debouncer, DebouncerPhase};

#[test]
fn debouncer_idle_to_ready_cycle() {
    let mut d = Debouncer::new(0);
    assert_eq!(d.phase(), DebouncerPhase::Idle);
    assert!(!d.ready());
    assert!(!d.pending());

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
    assert!(d.pending());
    assert!(!d.ready());
}
