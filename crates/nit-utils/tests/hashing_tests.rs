//! Integration tests for `nit_utils::hashing`.

use nit_utils::hashing::{stable_hash_bytes, SplitMix64};

#[test]
fn stable_hash_is_deterministic() {
    let a = stable_hash_bytes(b"test");
    let b = stable_hash_bytes(b"test");
    assert_eq!(a, b);
}

#[test]
fn stable_hash_differs_for_different_input() {
    let a = stable_hash_bytes(b"aaa");
    let b = stable_hash_bytes(b"bbb");
    assert_ne!(a, b);
}

#[test]
fn splitmix64_is_seedable_and_deterministic() {
    let mut a = SplitMix64::new(42);
    let mut b = SplitMix64::new(42);
    assert_eq!(a.next_u64(), b.next_u64());
    assert_eq!(a.next_u64(), b.next_u64());
}

#[test]
fn splitmix64_different_seeds_differ() {
    let mut a = SplitMix64::new(1);
    let mut b = SplitMix64::new(2);
    assert_ne!(a.next_u64(), b.next_u64());
}
