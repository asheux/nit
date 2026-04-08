use std::io::Write;
use std::path::PathBuf;

fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nit_{label}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn write_atomic_creates_file() {
    let dir = scratch_dir("fs_create");
    let target = dir.join("out.txt");

    nit_utils::fs::write_atomic(&target, |w| w.write_all(b"hello")).expect("write_atomic failed");

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    assert!(
        !dir.join("out.tmp").exists(),
        "temp file should be cleaned up"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn write_atomic_overwrites_existing() {
    let dir = scratch_dir("fs_overwrite");
    let target = dir.join("out.txt");
    std::fs::write(&target, b"old").unwrap();

    nit_utils::fs::write_atomic(&target, |w| w.write_all(b"new")).expect("write_atomic failed");

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn write_atomic_cleans_up_on_failure() {
    let dir = scratch_dir("fs_failure");
    let target = dir.join("fail.txt");

    let result = nit_utils::fs::write_atomic(&target, |_w| {
        Err(std::io::Error::new(std::io::ErrorKind::Other, "simulated"))
    });

    assert!(result.is_err());
    assert!(
        !target.exists(),
        "destination should not exist after failure"
    );
    assert!(
        !dir.join("fail.tmp").exists(),
        "temp file should be removed on failure"
    );
    std::fs::remove_dir_all(&dir).ok();
}
