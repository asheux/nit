use std::io::Write;
use std::path::PathBuf;

fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nit_{label}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct DirCleanup(PathBuf);

impl Drop for DirCleanup {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

#[test]
fn write_atomic_creates_file() {
    let dir = scratch_dir("fs_create");
    let _cleanup = DirCleanup(dir.clone());
    let target = dir.join("out.txt");

    nit_utils::write_atomic(&target, |w| w.write_all(b"hello")).expect("write_atomic failed");

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    assert!(
        !dir.join("out.tmp").exists(),
        "temp file should be cleaned up"
    );
}

#[test]
fn write_atomic_overwrites_existing() {
    let dir = scratch_dir("fs_overwrite");
    let _cleanup = DirCleanup(dir.clone());
    let target = dir.join("out.txt");
    std::fs::write(&target, b"old").unwrap();

    nit_utils::write_atomic(&target, |w| w.write_all(b"new")).expect("write_atomic failed");

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
}

#[test]
fn write_atomic_cleans_up_on_failure() {
    let dir = scratch_dir("fs_failure");
    let _cleanup = DirCleanup(dir.clone());
    let target = dir.join("fail.txt");

    let result = nit_utils::write_atomic(&target, |_w| Err(std::io::Error::other("simulated")));

    assert!(result.is_err());
    assert!(
        !target.exists(),
        "destination should not exist after failure"
    );
    assert!(
        !dir.join("fail.tmp").exists(),
        "temp file should be removed on failure"
    );
}

#[test]
fn ensure_dir_creates_and_returns_target() {
    let dir = scratch_dir("fs_ensure");
    let _cleanup = DirCleanup(dir.clone());
    let nested = dir.join("a").join("b");

    let returned = nit_utils::ensure_dir(&nested).expect("ensure_dir failed");
    assert!(returned.is_dir());
}

#[test]
fn ensure_dir_idempotent() {
    let dir = scratch_dir("fs_ensure_idem");
    let _cleanup = DirCleanup(dir.clone());
    let nested = dir.join("c");

    nit_utils::ensure_dir(&nested).expect("first call failed");
    nit_utils::ensure_dir(&nested).expect("second call should succeed on existing dir");
    assert!(nested.is_dir());
}
