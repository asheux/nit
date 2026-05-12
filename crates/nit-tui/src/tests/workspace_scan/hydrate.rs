use std::fs;
use std::path::PathBuf;

use crate::workspace_scan::WorkspaceScanRuntime;

use super::{cleanup, make_state, temp_workspace, write_file};

#[test]
fn hydrate_populates_state_from_disk_cache() {
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");
    let report = nit_core::compute_genome_report("fn main() {}\n", &file);
    nit_core::agent_bus::persist_genome_report(&root, &report);

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert!(
        state.genome_reports.contains_key(&file),
        "cache entry was not loaded into state"
    );
    assert_eq!(scan.pending_count(), 0);

    cleanup(&root);
}

#[test]
fn stale_cache_triggers_re_eval() {
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");

    let mut stale = nit_core::compute_genome_report("fn main() {}\n", &file);
    stale.timestamp_ms = 0;
    nit_core::agent_bus::persist_genome_report(&root, &stale);

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert_eq!(scan.pending_count(), 1);
    cleanup(&root);
}

#[test]
fn purges_deleted_files_on_launch() {
    let root = temp_workspace();
    let deleted_path = root.join("src").join("gone.rs");
    let phantom = nit_core::compute_genome_report("fn gone() {}\n", &deleted_path);
    nit_core::agent_bus::persist_genome_report(&root, &phantom);

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert!(
        !state.genome_reports.contains_key(&deleted_path),
        "deleted file report should have been purged from state"
    );
    cleanup(&root);
}

#[test]
fn ignored_dirs_are_skipped() {
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
        "only the non-ignored file should be queued"
    );
    assert!(real.exists());
    cleanup(&root);
}

#[test]
fn non_source_extensions_are_skipped() {
    let root = temp_workspace();
    write_file(&root, "src/lib.rs", "fn main() {}\n");
    write_file(&root, "src/assets/image.png", "\0\0\0\0");
    write_file(&root, "src/data.bin", "\0");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert_eq!(
        scan.pending_count(),
        1,
        "only the .rs source file should be queued"
    );
    cleanup(&root);
}

#[test]
fn respects_genome_context_disabled() {
    let root = temp_workspace();
    write_file(&root, "src/lib.rs", "fn main() {}\n");

    let mut state = make_state(root.clone());
    state.settings.genome.genome_context_enabled = false;

    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert_eq!(scan.pending_count(), 0);
    assert!(state.genome_reports.is_empty());
    cleanup(&root);
}

#[test]
fn hydrate_is_idempotent() {
    let root = temp_workspace();
    write_file(&root, "src/lib.rs", "fn main() {}\n");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);
    let first = scan.pending_count();
    scan.hydrate(&mut state);
    let second = scan.pending_count();

    assert!(scan.hydrated());
    assert_eq!(first, second, "re-hydrate should not re-queue files");
    cleanup(&root);
}

#[test]
fn empty_workspace_has_no_pending_work() {
    let root = temp_workspace();
    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert_eq!(scan.pending_count(), 0);
    assert!(!scan.is_scanning());
    let (done, total) = scan.progress();
    assert_eq!(done, 0);
    assert_eq!(total, 0);
    cleanup(&root);
}

#[test]
fn md_and_config_files_are_skipped_from_code_scan() {
    // The scan is code-only: md / toml / yaml / json / txt are tracked by
    // SOURCE_EXTENSIONS for buffer reloads but yield no tree-sitter signal
    // worth simulating.
    let root = temp_workspace();
    write_file(&root, "src/lib.rs", "fn main() {}\n");
    write_file(&root, "README.md", "# hi\n");
    write_file(&root, "Cargo.toml", "[package]\nname = \"x\"\n");
    write_file(&root, "ci/config.yaml", "k: v\n");
    write_file(&root, "data/spec.json", "{}\n");
    write_file(&root, "NOTES.txt", "notes\n");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert_eq!(scan.pending_count(), 1);
    cleanup(&root);
}

#[test]
fn hydrate_before_and_after_returns_different_progress() {
    let root = temp_workspace();
    for idx in 0..3 {
        write_file(
            &root,
            &format!("src/f{idx}.rs"),
            &format!("fn f{idx}() {{}}\n"),
        );
    }

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();

    let (d0, t0) = scan.progress();
    assert_eq!(d0, 0);
    assert_eq!(t0, 0);

    scan.hydrate(&mut state);
    let (d1, t1) = scan.progress();
    assert_eq!(d1, 0);
    assert_eq!(t1, 3);
    cleanup(&root);
}

#[test]
fn hydrate_seeds_all_files_even_when_reports_are_missing() {
    let root = temp_workspace();
    for idx in 0..5 {
        write_file(
            &root,
            &format!("crate_{idx}/lib.rs"),
            &format!("fn f{idx}() {{}}\n"),
        );
    }

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert_eq!(scan.pending_count(), 5);
    cleanup(&root);
}

#[test]
fn hidden_dot_dirs_are_skipped() {
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
fn state_gitignored_dirs_excludes_files_at_hydrate() {
    // The scan reads `state.gitignored_dirs` at hydrate time; adding a path
    // to that list must keep matching files out of the queue.
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

#[test]
fn single_file_launch_scans_parent_dir() {
    // Simulates `nit path/to/file.rs` — workspace_root is the file's parent;
    // the scan walks that parent for source files.
    let root = temp_workspace();
    let sub = root.join("project");
    fs::create_dir_all(&sub).unwrap();
    write_file(&sub, "a.rs", "fn a() {}\n");
    write_file(&sub, "b.rs", "fn b() {}\n");

    let mut state = make_state(sub.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert_eq!(scan.pending_count(), 2);
    cleanup(&root);
}

#[test]
fn sanity_pathbuf_compiles() {
    let _p: PathBuf = PathBuf::new();
}
