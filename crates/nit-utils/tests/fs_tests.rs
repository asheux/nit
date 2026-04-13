use std::io::Write;
use std::path::{Path, PathBuf};

struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("nit_{label}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

#[test]
fn write_atomic_creates_file() {
    let dir = ScratchDir::new("fs_create");
    let target = dir.path().join("out.txt");

    nit_utils::write_atomic(&target, |w| w.write_all(b"hello")).expect("write_atomic failed");

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    assert!(
        !dir.path().join("out.tmp").exists(),
        "temp file should be cleaned up"
    );
}

#[test]
fn write_atomic_overwrites_existing() {
    let dir = ScratchDir::new("fs_overwrite");
    let target = dir.path().join("out.txt");
    std::fs::write(&target, b"old").unwrap();

    nit_utils::write_atomic(&target, |w| w.write_all(b"new")).expect("write_atomic failed");

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
}

#[test]
fn write_atomic_cleans_up_on_failure() {
    let dir = ScratchDir::new("fs_failure");
    let target = dir.path().join("fail.txt");

    let result = nit_utils::write_atomic(&target, |_w| Err(std::io::Error::other("simulated")));

    assert!(result.is_err(), "should propagate callback error");
    assert!(
        !target.exists(),
        "destination should not exist after failure"
    );
    assert!(
        !dir.path().join("fail.tmp").exists(),
        "temp file should be removed on failure"
    );
}

#[test]
fn ensure_dir_creates_nested() {
    let dir = ScratchDir::new("fs_ensure");
    let nested = dir.path().join("a").join("b");

    let returned = nit_utils::ensure_dir(&nested).expect("ensure_dir failed");
    assert!(returned.is_dir(), "returned path should be a directory");
}

#[test]
fn ensure_dir_idempotent() {
    let dir = ScratchDir::new("fs_ensure_idem");
    let nested = dir.path().join("c");

    nit_utils::ensure_dir(&nested).expect("first call failed");
    nit_utils::ensure_dir(&nested).expect("second call should succeed on existing dir");
    assert!(nested.is_dir());
}
