use std::io::Write;
use std::path::{Path, PathBuf};

struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("nit_{label}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("failed to create scratch directory");
        Self(dir)
    }
}

impl std::ops::Deref for ScratchDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn has_tmp_residue(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .expect("read_dir")
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().contains(".tmp."))
}

#[test]
fn write_atomic_creates_file() {
    let dir = ScratchDir::new("fs_create");
    let target = dir.join("out.txt");

    nit_utils::write_atomic(&target, |w| w.write_all(b"hello")).expect("write_atomic failed");

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    assert!(!has_tmp_residue(&dir), "temp file should be cleaned up");
}

#[test]
fn write_atomic_overwrites_existing() {
    let dir = ScratchDir::new("fs_overwrite");
    let target = dir.join("out.txt");
    std::fs::write(&target, b"old").unwrap();

    nit_utils::write_atomic(&target, |w| w.write_all(b"new")).expect("write_atomic failed");

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
}

#[test]
fn write_atomic_cleans_up_on_failure() {
    let dir = ScratchDir::new("fs_failure");
    let target = dir.join("fail.txt");

    let result = nit_utils::write_atomic(&target, |_w| Err(std::io::Error::other("simulated")));

    assert!(result.is_err(), "should propagate callback error");
    assert!(
        !target.exists(),
        "destination should not exist after failure"
    );
    assert!(
        !has_tmp_residue(&dir),
        "temp file should be removed on failure"
    );
}

// Regression test for the old `path.with_extension("tmp")` behavior, which
// collapsed `foo.txt` and `foo.json` onto the same sibling path.
#[test]
fn write_atomic_preserves_unrelated_tmp_sibling() {
    let dir = ScratchDir::new("fs_sibling");
    let target = dir.join("out.txt");
    let bystander = dir.join("out.tmp");
    std::fs::write(&bystander, b"sentinel").unwrap();

    nit_utils::write_atomic(&target, |w| w.write_all(b"hello")).expect("write_atomic failed");

    assert_eq!(std::fs::read_to_string(&bystander).unwrap(), "sentinel");
}
