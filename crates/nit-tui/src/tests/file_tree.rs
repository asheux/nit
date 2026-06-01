use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use nit_utils::paths::{is_safe_leaf_name, path_within};

use super::{apply_mutation, FileTreeMutation};

fn temp_root() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nit-file-tree-{pid}-{ts}-{n}"));
    fs::create_dir_all(&dir).unwrap();
    // Canonicalise so the jail check isn't tripped by macOS's /var -> /private/var symlink.
    fs::canonicalize(&dir).unwrap_or(dir)
}

fn cleanup(root: &Path) {
    let _ = fs::remove_dir_all(root);
}

#[test]
fn safe_leaf_name_accepts_ordinary_names() {
    assert!(is_safe_leaf_name("foo.rs"));
    assert!(is_safe_leaf_name("my_module-2"));
    assert!(is_safe_leaf_name("a file.txt"));
    assert!(is_safe_leaf_name(".hidden"));
}

#[test]
fn safe_leaf_name_rejects_empty_traversal_and_separators() {
    for bad in [
        "",
        "   ",
        ".",
        "..",
        "a/b",
        "a\\b",
        "/etc/passwd",
        "../escape",
    ] {
        assert!(!is_safe_leaf_name(bad), "should reject {bad:?}");
    }
}

#[test]
fn path_within_accepts_direct_and_nested_children() {
    let root = temp_root();
    assert!(path_within(&root, &root.join("new.txt")));
    fs::create_dir(root.join("sub")).unwrap();
    assert!(path_within(&root, &root.join("sub").join("child.txt")));
    cleanup(&root);
}

#[test]
fn path_within_rejects_parent_escape_absolute_and_missing_parent() {
    let root = temp_root();
    assert!(!path_within(&root, &root.join("..").join("evil.txt")));
    assert!(!path_within(&root, Path::new("/etc/passwd")));
    // A name with an embedded separator lands under a non-existent dir, whose
    // canonicalize() fails -> refused.
    assert!(!path_within(&root, &root.join("missing").join("x.txt")));
    cleanup(&root);
}

#[cfg(unix)]
#[test]
fn path_within_rejects_symlink_escape() {
    let root = temp_root();
    let outside = temp_root();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    // parent = root/link canonicalises to `outside`, outside the jail.
    assert!(!path_within(&root, &root.join("link").join("evil.txt")));
    cleanup(&root);
    cleanup(&outside);
}

#[test]
fn apply_mutation_creates_file_and_dir() {
    let root = temp_root();
    apply_mutation(
        &root,
        &FileTreeMutation::CreateFile {
            path: root.join("a.txt"),
        },
    )
    .unwrap();
    assert!(root.join("a.txt").is_file());
    apply_mutation(
        &root,
        &FileTreeMutation::CreateDir {
            path: root.join("d"),
        },
    )
    .unwrap();
    assert!(root.join("d").is_dir());
    cleanup(&root);
}

#[test]
fn apply_mutation_renames_within_tree() {
    let root = temp_root();
    fs::write(root.join("old.txt"), b"x").unwrap();
    apply_mutation(
        &root,
        &FileTreeMutation::Rename {
            from: root.join("old.txt"),
            to: root.join("new.txt"),
        },
    )
    .unwrap();
    assert!(!root.join("old.txt").exists());
    assert!(root.join("new.txt").is_file());
    cleanup(&root);
}

#[test]
fn apply_mutation_rejects_collision() {
    let root = temp_root();
    fs::write(root.join("exists.txt"), b"x").unwrap();
    let err = apply_mutation(
        &root,
        &FileTreeMutation::CreateFile {
            path: root.join("exists.txt"),
        },
    )
    .unwrap_err();
    assert!(err.contains("already exists"), "got: {err}");
    cleanup(&root);
}

#[test]
fn apply_mutation_rejects_escape_without_writing() {
    let root = temp_root();
    let escape = root.join("..").join("nit-escape-must-not-exist.txt");
    let err = apply_mutation(
        &root,
        &FileTreeMutation::CreateFile {
            path: escape.clone(),
        },
    )
    .unwrap_err();
    assert!(err.contains("escapes the workspace"), "got: {err}");
    assert!(!escape.exists());
    cleanup(&root);
}
