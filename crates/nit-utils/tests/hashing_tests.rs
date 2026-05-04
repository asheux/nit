use nit_utils::{stable_hash_bytes, ContentTag, Fingerprint};

#[test]
fn stable_hash_invariants() {
    let test = stable_hash_bytes(b"test");
    assert_eq!(
        test,
        stable_hash_bytes(b"test"),
        "deterministic for same input"
    );
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
    assert_eq!(
        owned_vec.fingerprint(),
        [1u8, 2, 3].as_slice().fingerprint()
    );
}

#[test]
fn content_tag_display_and_field_semantics() {
    let tag = ContentTag::new("v2", b"payload");
    let display = tag.to_string();
    assert!(
        display.starts_with("v2-"),
        "expected v2- prefix, got {display}"
    );
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
