use nit_utils::hashing::stable_hash_bytes;
use nit_utils::{content_tag, ensure_dir, ContentTag, Fingerprint};

#[test]
fn fingerprint_byte_slice_matches_direct_hash() {
    let via_fn = stable_hash_bytes(b"hello");
    let via_trait: u64 = b"hello".as_slice().fingerprint();
    assert_eq!(via_fn, via_trait);
}

#[test]
fn fingerprint_str_matches_byte_hash() {
    assert_eq!("world".fingerprint(), stable_hash_bytes(b"world"));
}

#[test]
fn fingerprint_string_delegates_to_str() {
    let owned = String::from("test");
    assert_eq!(owned.fingerprint(), "test".fingerprint());
}

#[test]
fn fingerprint_vec_delegates_to_slice() {
    let owned = vec![1u8, 2, 3];
    assert_eq!(owned.fingerprint(), [1u8, 2, 3].as_slice().fingerprint());
}

#[test]
fn content_tag_format_and_determinism() {
    let tag = content_tag("v2", b"payload");
    assert!(tag.starts_with("v2-"), "expected v2- prefix, got {tag}");
    assert_eq!(tag.len(), "v2-".len() + 8);
    assert_eq!(tag, content_tag("v2", b"payload"), "must be deterministic");
}

#[test]
fn content_tag_struct_round_trips() {
    let tag = ContentTag::new("v2", b"payload");
    assert_eq!(tag.prefix(), "v2");
    assert_eq!(tag.to_string(), content_tag("v2", b"payload"));
}

#[test]
fn content_tag_digest_matches_across_prefixes() {
    let a = ContentTag::new("alpha", b"same");
    let b = ContentTag::new("beta", b"same");
    assert!(a.digest_matches(&b), "same payload should match");
    assert_ne!(a, b, "different prefixes means different tags");
}

#[test]
fn content_tag_different_payloads_differ() {
    let a = ContentTag::new("v1", b"aaa");
    let b = ContentTag::new("v1", b"bbb");
    assert_ne!(a.digest(), b.digest());
}

#[test]
fn ensure_dir_creates_and_returns_target() {
    let dir = std::env::temp_dir().join(format!("nit_lib_{}", std::process::id()));
    let _cleanup = DirCleanup(&dir);
    let returned = ensure_dir(&dir).expect("mkdir failed");
    assert!(returned.is_dir());
}

struct DirCleanup<'a>(&'a std::path::Path);

impl Drop for DirCleanup<'_> {
    fn drop(&mut self) {
        std::fs::remove_dir_all(self.0).ok();
    }
}
