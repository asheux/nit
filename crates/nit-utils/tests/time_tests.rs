use nit_utils::time::now_millis;

#[test]
fn now_millis_monotonic() {
    let a = now_millis();
    let b = now_millis();
    assert!(b >= a, "expected non-decreasing: got {a} then {b}");
}

// Subsumes a separate `> 0` check: a realistic post-2020 timestamp is trivially
// non-zero and also catches garbage fallbacks (e.g. accidental `0` on error).
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
