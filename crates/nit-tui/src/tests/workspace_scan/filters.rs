//! Path / extension filtering tests: gitignore conventions, ignored dirs,
//! non-source extensions, hidden dot-dirs. All run at hydrate time over a
//! synthetic workspace.

use crate::workspace_scan::WorkspaceScanRuntime;

use super::{cleanup, make_state, temp_workspace, write_file};

#[test]
fn ignored_dirs_are_skipped_at_hydrate() {
    let root = temp_workspace();
    write_file(&root, "target/debug/build.rs", "fn junk() {}\n");
    write_file(&root, "node_modules/pkg/index.js", "const x = 1;\n");
    write_file(&root, "vendor/time/lib.rs", "fn vendor() {}\n");
    let real = write_file(&root, "src/lib.rs", "fn main() {}\n");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert_eq!(
        scan.pending_count(),
        1,
        "only non-ignored file should be queued"
    );
    assert!(real.exists());
    cleanup(&root);
}

#[test]
fn non_source_extensions_filtered_out() {
    let root = temp_workspace();
    write_file(&root, "src/lib.rs", "fn main() {}\n");
    write_file(&root, "src/assets/image.png", "\0\0\0\0");
    write_file(&root, "src/data.bin", "\0");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert_eq!(scan.pending_count(), 1);
    cleanup(&root);
}

#[test]
fn hidden_dot_dirs_filtered() {
    let root = temp_workspace();
    write_file(&root, ".git/HEAD", "ref: refs/heads/main\n");
    write_file(&root, ".vscode/settings.json", "{}\n");
    write_file(&root, "src/lib.rs", "fn main() {}\n");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert_eq!(scan.pending_count(), 1);
    cleanup(&root);
}

#[test]
fn state_gitignored_dirs_filtered() {
    let root = temp_workspace();
    write_file(&root, "mine/secret/info.rs", "fn s() {}\n");
    write_file(&root, "src/lib.rs", "fn l() {}\n");

    let mut state = make_state(root.clone());
    state.gitignored_dirs = vec!["mine".into()];

    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert_eq!(scan.pending_count(), 1);
    cleanup(&root);
}
