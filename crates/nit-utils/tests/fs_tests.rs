//! Integration tests for `nit_utils::fs`.

use std::io::Write;

#[test]
fn write_atomic_creates_file() {
    let dir = std::env::temp_dir().join(format!("nit_fs_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("out.txt");

    nit_utils::fs::write_atomic(&target, |w| {
        w.write_all(b"hello")?;
        Ok(())
    })
    .expect("write_atomic failed");

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn write_atomic_overwrites_existing() {
    let dir = std::env::temp_dir().join(format!("nit_fs_overwrite_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("out.txt");
    std::fs::write(&target, b"old").unwrap();

    nit_utils::fs::write_atomic(&target, |w| {
        w.write_all(b"new")?;
        Ok(())
    })
    .expect("write_atomic failed");

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    std::fs::remove_dir_all(&dir).ok();
}
