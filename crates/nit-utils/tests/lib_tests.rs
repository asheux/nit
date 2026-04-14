use nit_utils::{stable_hash_bytes, ContentTag, Fingerprint};

#[test]
fn fingerprint_matches_direct_hash() {
    let via_fn = stable_hash_bytes(b"hello");
    let via_trait: u64 = b"hello".as_slice().fingerprint();
    assert_eq!(via_fn, via_trait);
}

#[test]
fn fingerprint_str_matches_bytes() {
    assert_eq!("world".fingerprint(), stable_hash_bytes(b"world"));
}

#[test]
fn fingerprint_string_matches_str() {
    let owned = String::from("test");
    assert_eq!(owned.fingerprint(), "test".fingerprint());
}

#[test]
fn fingerprint_vec_matches_slice() {
    let owned = vec![1u8, 2, 3];
    assert_eq!(owned.fingerprint(), [1u8, 2, 3].as_slice().fingerprint());
}

#[test]
fn content_tag_display_is_deterministic() {
    let tag = ContentTag::new("v2", b"payload");
    let display = tag.to_string();
    assert!(
        display.starts_with("v2-"),
        "expected v2- prefix, got {display}"
    );
    assert_eq!(display.len(), "v2-".len() + 8);
    assert_eq!(display, ContentTag::new("v2", b"payload").to_string());
}

#[test]
fn content_tag_accessors() {
    let tag = ContentTag::new("v2", b"payload");
    assert_eq!(tag.prefix(), "v2");
    assert_eq!(tag.digest(), ContentTag::new("v2", b"payload").digest());
}

#[test]
fn same_payload_same_digest_across_prefixes() {
    let a = ContentTag::new("alpha", b"same");
    let b = ContentTag::new("beta", b"same");
    assert_eq!(a.digest(), b.digest());
    assert_ne!(a, b);
}

#[test]
fn tag_different_payloads_differ() {
    let a = ContentTag::new("v1", b"aaa");
    let b = ContentTag::new("v1", b"bbb");
    assert_ne!(a.digest(), b.digest());
}
