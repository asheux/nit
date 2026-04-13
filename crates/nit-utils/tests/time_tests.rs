use nit_utils::time::now_millis;

#[test]
fn now_millis_nonzero() {
    assert!(now_millis() > 0);
}

#[test]
fn now_millis_monotonic() {
    let a = now_millis();
    let b = now_millis();
    assert!(b >= a, "expected non-decreasing: got {a} then {b}");
}

#[test]
fn now_millis_in_reasonable_range() {
    const JAN_2020_MS: u128 = 1_577_836_800_000;
    const JAN_2100_MS: u128 = 4_102_444_800_000;
    let now = now_millis();
    assert!(
        now > JAN_2020_MS && now < JAN_2100_MS,
        "timestamp {now} outside 2020..2100 range"
    );
}
