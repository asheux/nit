use nit_utils::{stable_hash_bytes, ContentTag, Fingerprint, SplitMix64};

#[test]
fn stable_hash_invariants() {
    let test = stable_hash_bytes(b"test");
    assert_eq!(test, stable_hash_bytes(b"test"), "deterministic for same input");
    assert_ne!(test, stable_hash_bytes(b"different"), "sensitive to input");
    let empty = stable_hash_bytes(b"");
    assert_eq!(empty, stable_hash_bytes(b""));
    assert_ne!(empty, 0, "empty input must not hash to zero");
}

#[test]
fn fingerprint_blanket_impl_agrees_across_byte_sources() {
    assert_eq!(
        b"hello".as_slice().fingerprint(),
        stable_hash_bytes(b"hello"),
    );
    assert_eq!("world".fingerprint(), stable_hash_bytes(b"world"));
    let owned_str = String::from("test");
    assert_eq!(owned_str.fingerprint(), "test".fingerprint());
    let owned_vec = vec![1u8, 2, 3];
    assert_eq!(owned_vec.fingerprint(), [1u8, 2, 3].as_slice().fingerprint());
}

#[test]
fn content_tag_display_and_field_semantics() {
    let tag = ContentTag::new("v2", b"payload");
    let display = tag.to_string();
    assert!(display.starts_with("v2-"), "expected v2- prefix, got {display}");
    assert_eq!(display.len(), "v2-".len() + 8);
    assert_eq!(display, ContentTag::new("v2", b"payload").to_string());
    assert_eq!(tag.prefix, "v2");
    assert_eq!(tag.digest, ContentTag::new("v2", b"payload").digest);

    let alpha = ContentTag::new("alpha", b"same");
    let beta = ContentTag::new("beta", b"same");
    assert_eq!(alpha.digest, beta.digest, "digest depends only on payload");
    assert_ne!(alpha, beta, "tags differ when prefixes differ");

    let differ_a = ContentTag::new("v1", b"aaa");
    let differ_b = ContentTag::new("v1", b"bbb");
    assert_ne!(differ_a.digest, differ_b.digest);
}

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
