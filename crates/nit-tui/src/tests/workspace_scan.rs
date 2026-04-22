//! Tests for the workspace-wide genome scan driver.
//!
//! Each test scaffolds a fresh `AppState` rooted at a temp directory and
//! drives the scan against real files. The `GenomeWorker` is used end-to-end
//! so `compute_genome_report` actually runs — these are integration tests
//! by design. The cost is acceptable because the scan is the single most
//! important change in this refactor and mocking the worker would hide the
//! state/cache invariants that mattered in practice.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nit_core::{AppState, Buffer, GenomeReport};

use crate::genome_worker::GenomeWorker;
use crate::workspace_scan::WorkspaceScanRuntime;

fn temp_workspace() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    // (pid, ts, counter) — the counter dominates because macOS's SystemTime
    // clock has coarser-than-nanosecond resolution, so parallel tests
    // calling `now()` in the same microsecond would otherwise collide on
    // the same directory name.
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nit-ws-scan-{pid}-{ts}-{n}"));
    fs::create_dir_all(&dir).unwrap();
    // Canonicalise so `strip_prefix` comparisons against workspace_root don't
    // trip on macOS's `/var` -> `/private/var` symlink.
    fs::canonicalize(&dir).unwrap_or(dir)
}

fn cleanup(root: &Path) {
    let _ = fs::remove_dir_all(root);
}

fn make_state(root: PathBuf) -> AppState {
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
    state.settings.genome.genome_context_enabled = true;
    state
}

fn write_file(root: &Path, rel: &str, contents: &str) -> PathBuf {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, contents).unwrap();
    path
}

fn drain_until_idle(
    state: &mut AppState,
    scan: &mut WorkspaceScanRuntime,
    worker: &GenomeWorker,
    max_wait: Duration,
) {
    let start = Instant::now();
    loop {
        scan.drive(worker);
        while let Ok(result) = worker.rx.try_recv() {
            assert!(result.workspace_scan, "non-scan result leaked");
            if let Some(report) = result.report {
                nit_core::agent_bus::persist_genome_report(&state.workspace_root, &report);
                state.genome_reports.insert(result.path.clone(), report);
            }
            scan.note_completed(&result.path);
        }
        if !scan.is_scanning() {
            return;
        }
        if start.elapsed() > max_wait {
            panic!("scan did not drain within {max_wait:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn hydrate_populates_state_from_disk_cache() {
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");
    // Pre-seed a cache entry without running the full scan.
    let report = nit_core::compute_genome_report("fn main() {}\n", &file);
    nit_core::agent_bus::persist_genome_report(&root, &report);

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert!(
        state.genome_reports.contains_key(&file),
        "cache entry was not loaded into state"
    );
    // Fresh cache hit for the only file → nothing to evaluate.
    assert_eq!(scan.pending_count(), 0);

    cleanup(&root);
}

#[test]
fn stale_cache_triggers_re_eval() {
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");

    // Cache entry with a zero timestamp — always stale relative to any file.
    let mut stale = nit_core::compute_genome_report("fn main() {}\n", &file);
    stale.timestamp_ms = 0;
    nit_core::agent_bus::persist_genome_report(&root, &stale);

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    // Stale cache should enqueue an eval for the file.
    assert_eq!(scan.pending_count(), 1);

    cleanup(&root);
}

#[test]
fn purges_deleted_files_on_launch() {
    let root = temp_workspace();
    let deleted_path = root.join("src").join("gone.rs");
    // Persist a report for a file that doesn't exist on disk.
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
    // Source file under target/ (IGNORED_DIRS) should NOT be queued.
    write_file(&root, "target/debug/build.rs", "fn junk() {}\n");
    write_file(&root, "node_modules/pkg/index.js", "const x = 1;\n");
    write_file(&root, "vendor/time/lib.rs", "fn vendor() {}\n");
    // Legitimate file that should be queued.
    let real = write_file(&root, "src/lib.rs", "fn main() {}\n");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    let pending = scan.pending_count();
    assert_eq!(pending, 1, "only the non-ignored file should be queued");
    assert!(real.exists());

    cleanup(&root);
}

#[test]
fn non_source_extensions_are_skipped() {
    let root = temp_workspace();
    write_file(&root, "src/lib.rs", "fn main() {}\n");
    // Non-source extensions that should be excluded by SOURCE_EXTENSIONS.
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
fn file_change_invalidates_cache_and_requeues() {
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");
    // Seed with a timestamp firmly in the past so the file's (now) mtime
    // beats it — simulating a real edit since the cache was last written.
    let mut report = nit_core::compute_genome_report("fn main() {}\n", &file);
    report.timestamp_ms = 0;
    nit_core::agent_bus::persist_genome_report(&root, &report);

    let mut state = make_state(root.clone());
    state.genome_reports.insert(file.clone(), report);

    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, file.clone());

    assert!(
        !state.genome_reports.contains_key(&file),
        "change event should invalidate the cached report"
    );
    assert_eq!(
        scan.pending_count(),
        1,
        "change event should enqueue a re-eval"
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

    assert_eq!(
        scan.pending_count(),
        0,
        "scan should not queue anything when genome context is disabled"
    );
    assert!(
        state.genome_reports.is_empty(),
        "no cache should be loaded when genome context is disabled"
    );

    cleanup(&root);
}

#[test]
fn single_file_launch_scans_parent_dir() {
    // Simulates `nit path/to/file.rs` — the workspace_root is the file's
    // parent. The scan should walk that parent directory for source files.
    let root = temp_workspace();
    let sub = root.join("project");
    fs::create_dir_all(&sub).unwrap();
    write_file(&sub, "a.rs", "fn a() {}\n");
    write_file(&sub, "b.rs", "fn b() {}\n");

    let mut state = make_state(sub.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert_eq!(
        scan.pending_count(),
        2,
        "both source files in the parent dir should be queued"
    );

    cleanup(&root);
}

#[test]
fn scan_processes_many_files_end_to_end() {
    // Integration test: many files, all evaluated via the real genome worker.
    // Verifies the in-flight cap doesn't deadlock and every report lands in
    // state when the scan drains.
    let root = temp_workspace();
    let file_count = 16;
    for idx in 0..file_count {
        write_file(
            &root,
            &format!("src/file_{idx:03}.rs"),
            &format!("fn f{idx}() {{ let _ = {idx}; }}\n"),
        );
    }

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    let worker = GenomeWorker::new();

    scan.hydrate(&mut state);
    assert_eq!(scan.pending_count(), file_count);

    drain_until_idle(&mut state, &mut scan, &worker, Duration::from_secs(30));

    assert_eq!(state.genome_reports.len(), file_count);
    let (done, total) = scan.progress();
    assert_eq!(done, file_count);
    assert_eq!(total, file_count);

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
fn change_event_outside_workspace_is_ignored() {
    let root = temp_workspace();
    let outside = std::env::temp_dir().join("nit-ws-scan-outside.rs");
    fs::write(&outside, "fn outside() {}\n").unwrap();

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, outside.clone());

    assert_eq!(
        scan.pending_count(),
        0,
        "paths outside workspace_root must be dropped"
    );

    let _ = fs::remove_file(&outside);
    cleanup(&root);
}

#[test]
fn change_event_in_gitignored_dir_is_ignored() {
    let root = temp_workspace();
    // Mimic a gitignored dir entry.
    let file = write_file(&root, "target/debug/foo.rs", "fn foo() {}\n");

    let mut state = make_state(root.clone());
    state.gitignored_dirs = vec!["target".into()];

    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, file.clone());

    assert_eq!(
        scan.pending_count(),
        0,
        "changes under ignored dirs must not enqueue"
    );

    cleanup(&root);
}

#[test]
fn delete_event_drops_cached_report_and_disk_file() {
    let root = temp_workspace();
    let file_path = root.join("src/lib.rs");
    // Pretend the file was deleted: write the cache but not the file.
    let mut report = GenomeReport {
        file_path: file_path.clone(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.0,
        tier: nit_core::GenomeTier::StillLife,
        recommendations: Vec::new(),
        timestamp_ms: 0,
        grid_size: 32,
        parsimony: Default::default(),
    };
    report.timestamp_ms = 1;
    nit_core::agent_bus::persist_genome_report(&root, &report);

    let mut state = make_state(root.clone());
    state.genome_reports.insert(file_path.clone(), report);

    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, file_path.clone());

    assert!(
        !state.genome_reports.contains_key(&file_path),
        "deleted file should be purged from state"
    );

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
fn drive_respects_in_flight_cap() {
    let root = temp_workspace();
    // Seed well over the cap so we can observe the cap behavior.
    for idx in 0..32 {
        write_file(
            &root,
            &format!("src/f{idx}.rs"),
            &format!("fn f{idx}() {{}}\n"),
        );
    }

    let mut state = make_state(root.clone());
    // Pin the cap to 3 so the assertion is stable across hosts regardless of
    // the machine's available_parallelism.
    let mut scan = WorkspaceScanRuntime::with_max_in_flight(3);
    let worker = GenomeWorker::new();

    scan.hydrate(&mut state);
    // One drive pass should dispatch at most the configured cap before any
    // results have landed.
    scan.drive(&worker);
    assert!(
        scan.dispatched_count() <= 3,
        "drive dispatched more than the in-flight cap"
    );

    drain_until_idle(&mut state, &mut scan, &worker, Duration::from_secs(30));
    cleanup(&root);
}

#[test]
fn md_and_config_files_are_skipped_from_code_scan() {
    // The genome scan is code-only: markdown, toml, yaml, json, txt all
    // live in the broader SOURCE_EXTENSIONS (for buffer-reload tracking)
    // but give no signal under tree-sitter + GoL, so they must not be
    // queued for evaluation.
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

    assert_eq!(
        scan.pending_count(),
        1,
        "only src/lib.rs is code; md/toml/yaml/json/txt must be excluded"
    );

    cleanup(&root);
}

#[test]
fn workspace_scan_max_in_flight_is_positive_and_bounded() {
    let cap = crate::workspace_scan::workspace_scan_max_in_flight();
    assert!(cap >= 4, "cap must give the scan meaningful parallelism");
    assert!(cap <= 16, "cap must not exceed the hard ceiling");
}

#[test]
fn is_code_file_predicate_covers_expected_languages() {
    use crate::workspace_scan::is_code_file;
    for ext in ["rs", "py", "ts", "tsx", "go", "java", "c", "cpp", "swift"] {
        let p = PathBuf::from(format!("main.{ext}"));
        assert!(is_code_file(&p), "{ext} should be code");
    }
    for ext in ["md", "toml", "yaml", "yml", "json", "txt"] {
        let p = PathBuf::from(format!("x.{ext}"));
        assert!(!is_code_file(&p), "{ext} must be excluded");
    }
}

#[test]
fn note_change_skips_discovery_when_cache_is_fresh() {
    // File watcher emits a "discovered" event for every source file on
    // startup — not just real mtime changes. A fresh cache entry (report
    // timestamp > file mtime) must not be invalidated by this event, or
    // the scan re-runs from scratch on every launch.
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");

    let mut state = make_state(root.clone());

    // Seed a report with a timestamp well into the future so it's
    // guaranteed newer than the file's mtime.
    let report = GenomeReport {
        file_path: file.clone(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.0,
        tier: nit_core::GenomeTier::StillLife,
        recommendations: Vec::new(),
        timestamp_ms: u64::MAX,
        grid_size: 32,
        parsimony: Default::default(),
    };
    state.genome_reports.insert(file.clone(), report);

    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, file.clone());

    assert_eq!(
        scan.pending_count(),
        0,
        "fresh cache entry must not be invalidated by a discovery event"
    );
    assert!(
        state.genome_reports.contains_key(&file),
        "cached report must still be present"
    );

    cleanup(&root);
}

#[test]
fn note_change_requeues_when_file_is_actually_newer() {
    // Complement to the fresh-cache test: when the file mtime moves past
    // the report timestamp, the change event IS a real edit and must
    // invalidate + enqueue.
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");

    let mut state = make_state(root.clone());

    // Seed with timestamp = 0 so the file mtime beats it.
    let report = GenomeReport {
        file_path: file.clone(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.0,
        tier: nit_core::GenomeTier::StillLife,
        recommendations: Vec::new(),
        timestamp_ms: 0,
        grid_size: 32,
        parsimony: Default::default(),
    };
    state.genome_reports.insert(file.clone(), report);

    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, file.clone());

    assert_eq!(
        scan.pending_count(),
        1,
        "stale cache entry with newer file mtime must be re-queued"
    );

    cleanup(&root);
}

#[test]
fn note_change_ignores_non_code_extensions() {
    let root = temp_workspace();
    let md = write_file(&root, "README.md", "# doc\n");
    let json = write_file(&root, "pkg.json", "{}\n");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, md);
    scan.note_change(&mut state, json);

    assert_eq!(
        scan.pending_count(),
        0,
        "non-code change events must not enqueue"
    );

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

    // Before hydrate: total = 0.
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
fn note_change_with_unknown_extension_is_noop() {
    let root = temp_workspace();
    let file = write_file(&root, "assets/data.bin", "\0");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, file);

    assert_eq!(scan.pending_count(), 0);
}

#[test]
fn note_change_dedups_in_flight_path() {
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();

    scan.note_change(&mut state, file.clone());
    scan.note_change(&mut state, file.clone());

    assert_eq!(
        scan.pending_count(),
        1,
        "two identical change events must not double-queue"
    );

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
    // Files inside .git, .vscode, etc. should be skipped.
    write_file(&root, ".git/HEAD", "ref: refs/heads/main\n");
    write_file(&root, ".vscode/settings.json", "{}\n");
    // Legit file.
    write_file(&root, "src/lib.rs", "fn main() {}\n");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert_eq!(
        scan.pending_count(),
        1,
        "only src/lib.rs should be queued; hidden dirs must be skipped"
    );

    cleanup(&root);
}

#[test]
fn change_event_path_not_under_workspace_returns_early() {
    let root = temp_workspace();
    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();

    // Construct an absolute path outside the workspace root.
    let outside = PathBuf::from("/definitely/not/in/the/workspace/lib.rs");
    scan.note_change(&mut state, outside);

    assert_eq!(scan.pending_count(), 0);

    cleanup(&root);
}

// Verify that a workspace_scan result (workspace_scan: true) is produced by
// the new evaluate_from_disk_workspace_scan call, and that the drive+drain
// loop properly records completion.
#[test]
fn worker_result_carries_workspace_scan_flag() {
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");

    let worker = GenomeWorker::new();
    assert!(worker.evaluate_from_disk_workspace_scan(file.clone()));

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(result) = worker.rx.try_recv() {
            assert!(result.workspace_scan);
            assert!(!result.shadow);
            assert!(!result.save_eval);
            assert_eq!(result.path, file);
            assert!(result.report.is_some(), "report should have been computed");
            break;
        }
        if Instant::now() >= deadline {
            panic!("worker did not produce a result within 10s");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    cleanup(&root);
}

// Ensure the walker used by the scan walks `state.gitignored_dirs`. Since
// the scan reads them from state at hydrate time, adding to that list must
// keep the matching dir's files out of the queue.
#[test]
fn state_gitignored_dirs_excludes_files_at_hydrate() {
    let root = temp_workspace();
    write_file(&root, "mine/secret/info.rs", "fn s() {}\n");
    write_file(&root, "src/lib.rs", "fn l() {}\n");

    let mut state = make_state(root.clone());
    state.gitignored_dirs = vec!["mine".into()];

    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert_eq!(
        scan.pending_count(),
        1,
        "gitignored_dirs entry should exclude the secret file"
    );

    cleanup(&root);
}

// Round-trip: persist, delete, hydrate. Confirms delete_genome_report removes
// the cache file and hydrate finds nothing to load.
#[test]
fn persist_then_delete_round_trip() {
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");
    let report = nit_core::compute_genome_report("fn main() {}\n", &file);
    nit_core::agent_bus::persist_genome_report(&root, &report);

    let map = nit_core::agent_bus::load_genome_reports(&root);
    assert!(map.contains_key(&file));

    nit_core::agent_bus::delete_genome_report(&root, &file);
    let map_after = nit_core::agent_bus::load_genome_reports(&root);
    assert!(
        !map_after.contains_key(&file),
        "cache entry should be gone after delete"
    );

    cleanup(&root);
}

// Smoke test: HashSet import path doesn't regress.
#[test]
fn sanity_hashset_compiles() {
    let _s: HashSet<PathBuf> = HashSet::new();
}

#[test]
fn gate_monitor_tab_clicks_set_target_sub_view_directly() {
    // Regression: the three tabs (STATS / FILESCORES / LIVE) used to share
    // a single cycle action, so clicking a non-adjacent tab from the current
    // view stepped once through the cycle rather than jumping to the clicked
    // target. Verify each button returns the correct direct-set action and
    // that applying it from any starting state lands on the requested tab.
    use crate::widgets::gate_monitor_view::title_button_hit;
    use nit_core::{Action, GateMonitorSubView};

    // Title prefix is always " CODE STRUCTURAL QUALITY " (25 bytes) when no
    // genome report is rendered — mirror that layout in the test.
    let prefix = " CODE STRUCTURAL QUALITY ".len() as u16;

    // STATS button spans [prefix+1 .. prefix+1+7].
    // FILESCORES spans the next window; LIVE spans the one after.
    // Sample one column inside each button.
    let stats_col = prefix + 1 + 3; // middle of " STATS "
    let fs_col = prefix + 1 + 7 + 1 + 5; // middle of " FILESCORES "
    let live_col = prefix + 1 + 7 + 1 + 12 + 1 + 2; // middle of " LIVE "

    // Offset by +1 because title_button_hit subtracts the border byte.
    assert_eq!(
        title_button_hit(stats_col + 1, prefix),
        Some(Action::GateMonitorSetSubView(GateMonitorSubView::Stats))
    );
    assert_eq!(
        title_button_hit(fs_col + 1, prefix),
        Some(Action::GateMonitorSetSubView(
            GateMonitorSubView::FileScores
        ))
    );
    assert_eq!(
        title_button_hit(live_col + 1, prefix),
        Some(Action::GateMonitorSetSubView(GateMonitorSubView::Live))
    );

    // Driving the action from each starting state must land on the
    // requested tab — NOT cycle past it.
    let root = temp_workspace();
    let mut state = make_state(root.clone());

    state.gate_monitor_sub_view = GateMonitorSubView::Stats;
    nit_core::apply_action(
        &mut state,
        Action::GateMonitorSetSubView(GateMonitorSubView::Live),
    );
    assert_eq!(state.gate_monitor_sub_view, GateMonitorSubView::Live);

    state.gate_monitor_sub_view = GateMonitorSubView::Live;
    nit_core::apply_action(
        &mut state,
        Action::GateMonitorSetSubView(GateMonitorSubView::Stats),
    );
    assert_eq!(state.gate_monitor_sub_view, GateMonitorSubView::Stats);

    state.gate_monitor_sub_view = GateMonitorSubView::Live;
    nit_core::apply_action(
        &mut state,
        Action::GateMonitorSetSubView(GateMonitorSubView::FileScores),
    );
    assert_eq!(state.gate_monitor_sub_view, GateMonitorSubView::FileScores);

    cleanup(&root);
}

#[test]
fn in_flight_snapshot_separates_evaluating_from_queued() {
    use crate::workspace_scan::WorkspaceScanItemState;

    let root = temp_workspace();
    // Seed enough files to push some into pending and some into dispatched.
    for idx in 0..6 {
        write_file(
            &root,
            &format!("src/f{idx}.rs"),
            &format!("fn f{idx}() {{}}\n"),
        );
    }

    let mut state = make_state(root.clone());
    // Cap at 2 so we know exactly how many end up in dispatched after one drive.
    let mut scan = WorkspaceScanRuntime::with_max_in_flight(2);
    let worker = GenomeWorker::new();

    scan.hydrate(&mut state);
    assert_eq!(scan.pending_count(), 6);

    scan.drive(&worker);
    let snapshot = scan.in_flight_snapshot();
    assert_eq!(
        snapshot.len(),
        6,
        "every pending or dispatched file should appear"
    );
    let eval_count = snapshot
        .iter()
        .filter(|(_, s)| matches!(s, WorkspaceScanItemState::Evaluating))
        .count();
    let queued_count = snapshot
        .iter()
        .filter(|(_, s)| matches!(s, WorkspaceScanItemState::Queued))
        .count();
    assert_eq!(eval_count, 2, "drive should fill up to the cap");
    assert_eq!(queued_count, 4);

    drain_until_idle(&mut state, &mut scan, &worker, Duration::from_secs(30));
    assert!(scan.in_flight_snapshot().is_empty(), "drains to empty");

    cleanup(&root);
}

#[test]
fn in_flight_snapshot_is_empty_when_idle() {
    let root = temp_workspace();
    let state = make_state(root.clone());
    let scan = WorkspaceScanRuntime::new();
    let _ = state;
    assert!(scan.in_flight_snapshot().is_empty());
    cleanup(&root);
}

#[test]
fn session_touched_persists_after_completion() {
    // LIVE view relies on session_touched outliving in_flight_snapshot so
    // evaluated files don't vanish the moment their result lands. Seed a
    // few files, drain the scan, and confirm session_touched still lists
    // every path while in_flight is empty.
    let root = temp_workspace();
    for idx in 0..4 {
        write_file(
            &root,
            &format!("src/f{idx}.rs"),
            &format!("fn f{idx}() {{}}\n"),
        );
    }

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    let worker = GenomeWorker::new();

    scan.hydrate(&mut state);
    assert_eq!(scan.session_touched().len(), 4);

    drain_until_idle(&mut state, &mut scan, &worker, Duration::from_secs(30));

    assert!(
        scan.in_flight_snapshot().is_empty(),
        "in-flight snapshot drains on completion"
    );
    assert_eq!(
        scan.session_touched().len(),
        4,
        "session_touched must persist completed paths for the LIVE log"
    );

    cleanup(&root);
}

#[test]
fn note_change_of_new_file_is_tracked_in_session() {
    // A mid-session file edit (no cache, or stale cache) must enter
    // session_touched so the operator sees it in LIVE even after its
    // eval completes.
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, file.clone());

    assert!(scan.session_touched().contains(&file));

    cleanup(&root);
}

#[test]
fn note_change_skip_path_does_not_touch_session() {
    // When a fresh cache and a pre-session mtime combine to skip
    // invalidation (the discovery-burst case), the path should NOT be
    // recorded as session-touched — nothing actually got queued.
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");

    let mut state = make_state(root.clone());
    let report = GenomeReport {
        file_path: file.clone(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.0,
        tier: nit_core::GenomeTier::StillLife,
        recommendations: Vec::new(),
        // `u64::MAX` forces the mtime-vs-cache check to favor the cache,
        // AND the file mtime is well below session_start_ms since the
        // test created the file before the scan's construction.
        timestamp_ms: u64::MAX,
        grid_size: 32,
        parsimony: Default::default(),
    };
    state.genome_reports.insert(file.clone(), report);

    // Sleep briefly before constructing so session_start_ms is strictly
    // after the file mtime — the skip path is only taken when
    // `mtime < session_start_ms`.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, file.clone());

    assert_eq!(scan.pending_count(), 0);
    assert!(
        !scan.session_touched().contains(&file),
        "skipped discovery events must not bloat session log"
    );

    cleanup(&root);
}

#[test]
fn in_flight_snapshot_orders_evaluating_first() {
    use crate::workspace_scan::WorkspaceScanItemState;

    let root = temp_workspace();
    for idx in 0..4 {
        write_file(
            &root,
            &format!("src/f{idx}.rs"),
            &format!("fn f{idx}() {{}}\n"),
        );
    }

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::with_max_in_flight(2);
    let worker = GenomeWorker::new();

    scan.hydrate(&mut state);
    scan.drive(&worker);

    let snapshot = scan.in_flight_snapshot();
    // All Evaluating entries must precede all Queued ones so the LIVE view
    // renders active work at the top of the list.
    let mut saw_queued = false;
    for (_, state) in &snapshot {
        match state {
            WorkspaceScanItemState::Queued => saw_queued = true,
            WorkspaceScanItemState::Evaluating => {
                assert!(
                    !saw_queued,
                    "Evaluating entry appeared after a Queued entry"
                );
            }
        }
    }

    drain_until_idle(&mut state, &mut scan, &worker, Duration::from_secs(30));
    cleanup(&root);
}
