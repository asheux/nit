use nit_utils::time::now_millis;

#[test]
fn nonzero() {
    assert!(now_millis() > 0);
}

#[test]
fn monotonic() {
    let a = now_millis();
    let b = now_millis();
    assert!(b >= a, "timestamps should be monotonic: {a} > {b}");
}

#[test]
fn in_reasonable_range() {
    const EPOCH_2020: u128 = 1_577_836_800_000;
    const EPOCH_2100: u128 = 4_102_444_800_000;
    let now = now_millis();
    assert!(
        now > EPOCH_2020 && now < EPOCH_2100,
        "timestamp {now} outside 2020..2100 range"
    );
}
