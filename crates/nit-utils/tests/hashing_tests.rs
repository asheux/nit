use nit_utils::{stable_hash_bytes, SplitMix64};

#[test]
fn stable_hash_deterministic_same_input() {
    let a = stable_hash_bytes(b"test");
    let b = stable_hash_bytes(b"test");
    assert_eq!(a, b);
}

#[test]
fn stable_hash_differs_for_different_input() {
    assert_ne!(stable_hash_bytes(b"aaa"), stable_hash_bytes(b"bbb"));
}

#[test]
fn stable_hash_empty_input_is_stable() {
    let a = stable_hash_bytes(b"");
    let b = stable_hash_bytes(b"");
    assert_eq!(a, b);
    assert_ne!(a, 0, "empty input should not hash to zero");
}

#[test]
fn rng_deterministic_same_seed() {
    let mut a = SplitMix64::new(42);
    let mut b = SplitMix64::new(42);
    for _ in 0..10 {
        assert_eq!(a.next_u64(), b.next_u64());
    }
}

#[test]
fn rng_different_seeds_diverge() {
    let mut a = SplitMix64::new(1);
    let mut b = SplitMix64::new(2);
    assert_ne!(a.next_u64(), b.next_u64());
}

#[test]
fn rng_zero_seed_produces_output() {
    let mut rng = SplitMix64::new(0);
    let first = rng.next_u64();
    let second = rng.next_u64();
    assert_ne!(first, 0);
    assert_ne!(first, second);
}

#[test]
fn rng_bounded_stays_in_range() {
    let mut rng = SplitMix64::new(99);
    for _ in 0..200 {
        assert!(rng.next_bounded(10) < 10);
    }
}

#[test]
fn rng_bounded_upper_one_returns_zero() {
    let mut rng = SplitMix64::new(7);
    assert_eq!(rng.next_bounded(1), 0);
    assert_eq!(rng.next_bounded(0), 0);
}

#[test]
fn rng_f32_in_unit_interval() {
    let mut rng = SplitMix64::new(12345);
    for _ in 0..200 {
        let v = rng.next_f32();
        assert!((0.0..1.0).contains(&v), "got {v} outside [0.0, 1.0)");
    }
}

#[test]
fn rng_iterator_yields_values() {
    let rng = SplitMix64::new(77);
    let vals: Vec<u64> = rng.take(5).collect();
    assert_eq!(vals.len(), 5);
    assert_ne!(vals[0], vals[1]);
}
