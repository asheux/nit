use nit_utils::SplitMix64;

#[test]
fn rng_seed_semantics() {
    let mut left = SplitMix64::new(42);
    let mut right = SplitMix64::new(42);
    for _ in 0..10 {
        assert_eq!(left.next_u64(), right.next_u64());
    }

    let mut s1 = SplitMix64::new(1);
    let mut s2 = SplitMix64::new(2);
    assert_ne!(s1.next_u64(), s2.next_u64());

    // Seed 0 substitutes a nonzero default; output must still vary.
    let mut zero = SplitMix64::new(0);
    let first = zero.next_u64();
    assert_ne!(first, 0);
    assert_ne!(first, zero.next_u64());
}

#[test]
fn rng_bounded_outputs_stay_in_range() {
    let mut rng = SplitMix64::new(99);
    for _ in 0..200 {
        let drawn = rng.next_bounded(10);
        assert!(drawn < 10, "next_bounded(10) returned {drawn}");
    }
    // Upper bounds 0 and 1 both collapse to the only valid output, 0.
    assert_eq!(rng.next_bounded(1), 0);
    assert_eq!(rng.next_bounded(0), 0);
}

#[test]
fn rng_f32_in_unit_interval() {
    let mut rng = SplitMix64::new(12345);
    for _ in 0..200 {
        let v = rng.next_f32();
        assert!((0.0..1.0).contains(&v), "got {v} outside [0.0, 1.0)");
        assert!(v.is_finite(), "got non-finite f32: {v}");
    }
}

#[test]
fn rng_iterator_yields_distinct_values() {
    use std::collections::HashSet;
    let rng = SplitMix64::new(77);
    let vals: Vec<u64> = rng.take(5).collect();
    let unique: HashSet<u64> = vals.iter().copied().collect();
    assert_eq!(
        unique.len(),
        vals.len(),
        "expected 5 distinct draws: {vals:?}"
    );
}

#[test]
fn rng_clone_yields_identical_sequence() {
    let mut original = SplitMix64::new(0xDEAD_BEEF);
    // Burn a draw so the clone snapshots a non-initial state.
    original.next_u64();
    let mut copy = original.clone();
    for _ in 0..32 {
        assert_eq!(original.next_u64(), copy.next_u64());
    }
}

// Boundary check for the rejection-sampling threshold: with `upper = u64::MAX`,
// `upper.wrapping_neg() % upper` collapses to 1, so only the single value `0`
// is rejected. Verifies the loop terminates and the result stays below the
// bound.
#[test]
fn rng_next_bounded_max_u64_boundary() {
    let mut rng = SplitMix64::new(0x5EED_5EED);
    for _ in 0..64 {
        let drawn = rng.next_bounded(u64::MAX);
        assert!(drawn < u64::MAX);
    }
}
