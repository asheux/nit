//! Async directory walker that powers the per-pane dir-search overlay.
//!
//! Mirrors the shape of [`crate::fuzzy_search_runner::FuzzyMatcherRunner`]:
//! a single named worker thread, crossbeam channels for commands and
//! events, and an `Arc<AtomicU64>` request-id latch so a fresh
//! keystroke supersedes any in-flight walk. The worker calls
//! [`std::fs::read_dir`] at depth 1 (the spec caps the v1 walk at the
//! immediate children of `base`) and ranks the surviving entries with
//! [`super::dir_search::rank`], which delegates to
//! [`crate::fuzzy_search_runner::fuzzy_score_bytes`].
//!
//! Hidden files (anything starting with `.`) plus a fixed deny-list
//! of heavyweight build dirs (`node_modules`, `target`, `.venv`,
//! `dist`, `build`) are skipped unless the operator flips the
//! per-pane `f` toggle.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{unbounded, Receiver, Sender};

use super::dir_search::rank;

const MAX_RESULTS: usize = 50;
const HEAVY_DENY: &[&str] = &["node_modules", "target", ".venv", "dist", "build"];

#[derive(Debug)]
pub enum DirSearchCommand {
    Query {
        request_id: u64,
        base: PathBuf,
        needle: String,
        show_hidden: bool,
    },
    Shutdown,
}

#[derive(Debug)]
pub enum DirSearchEvent {
    Results {
        request_id: u64,
        base: PathBuf,
        results: Vec<PathBuf>,
    },
}

pub struct DirSearchRunner {
    cmd_tx: Sender<DirSearchCommand>,
    pub events: Receiver<DirSearchEvent>,
    handle: Option<JoinHandle<()>>,
    active_request: Arc<AtomicU64>,
    next_request_id: AtomicU64,
}

impl DirSearchRunner {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let active = Arc::new(AtomicU64::new(0));
        let active_for_worker = Arc::clone(&active);
        let handle = thread::Builder::new()
            .name("nit-multipane-dirsearch".into())
            .spawn(move || run_worker(cmd_rx, event_tx, active_for_worker))
            .expect("spawn nit-multipane-dirsearch");
        Self {
            cmd_tx,
            events: event_rx,
            handle: Some(handle),
            active_request: active,
            next_request_id: AtomicU64::new(0),
        }
    }

    /// Send a query. Returns the request id assigned to it; older
    /// in-flight walks become stale and their results are dropped.
    pub fn query(&self, base: PathBuf, needle: String, show_hidden: bool) -> u64 {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.active_request.store(id, Ordering::Relaxed);
        let _ = self.cmd_tx.send(DirSearchCommand::Query {
            request_id: id,
            base,
            needle,
            show_hidden,
        });
        id
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(DirSearchCommand::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for DirSearchRunner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_worker(
    cmd_rx: Receiver<DirSearchCommand>,
    event_tx: Sender<DirSearchEvent>,
    active: Arc<AtomicU64>,
) {
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            DirSearchCommand::Shutdown => break,
            DirSearchCommand::Query {
                request_id,
                base,
                needle,
                show_hidden,
            } => {
                if active.load(Ordering::Relaxed) != request_id {
                    continue;
                }
                let results = walk_one_level(&base, &needle, show_hidden, &active, request_id);
                if active.load(Ordering::Relaxed) != request_id {
                    continue;
                }
                let _ = event_tx.send(DirSearchEvent::Results {
                    request_id,
                    base,
                    results,
                });
            }
        }
    }
    active.store(u64::MAX, Ordering::Relaxed);
}

fn walk_one_level(
    base: &std::path::Path,
    needle: &str,
    show_hidden: bool,
    active: &AtomicU64,
    request_id: u64,
) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut scored: Vec<(i64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        if active.load(Ordering::Relaxed) != request_id {
            return Vec::new();
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !show_hidden && is_skipped(&name_str) {
            continue;
        }
        let path = entry.path();
        if needle.is_empty() {
            scored.push((0, path));
            continue;
        }
        if let Some(score) = rank(&name_str, needle) {
            scored.push((score, path));
        }
    }
    if needle.is_empty() {
        scored.sort_by(|a, b| a.1.file_name().cmp(&b.1.file_name()));
    } else {
        scored.sort_by(|a, b| b.0.cmp(&a.0));
    }
    scored.truncate(MAX_RESULTS);
    scored.into_iter().map(|(_, p)| p).collect()
}

fn is_skipped(name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    HEAVY_DENY.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            let nonce: u128 = Instant::now().elapsed().as_nanos();
            p.push(format!("nit-mp-dirsearch-{tag}-{nonce}"));
            fs::create_dir_all(&p).unwrap();
            Self { path: p }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn drain_until<F: Fn(&DirSearchEvent) -> bool>(
        rx: &Receiver<DirSearchEvent>,
        deadline: Duration,
        pred: F,
    ) -> Option<DirSearchEvent> {
        let start = Instant::now();
        while start.elapsed() < deadline {
            if let Ok(ev) = rx.recv_timeout(Duration::from_millis(50)) {
                if pred(&ev) {
                    return Some(ev);
                }
            }
        }
        None
    }

    #[test]
    fn walks_one_level_deep() {
        let tmp = TempDir::new("walk1");
        fs::create_dir(tmp.path.join("alpha")).unwrap();
        fs::create_dir(tmp.path.join("beta")).unwrap();
        fs::create_dir(tmp.path.join("alpha").join("nested")).unwrap();

        let runner = DirSearchRunner::spawn();
        runner.query(tmp.path.clone(), String::new(), false);
        let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
            matches!(ev, DirSearchEvent::Results { .. })
        })
        .expect("results");
        let DirSearchEvent::Results { results, .. } = evt;
        let names: Vec<String> = results
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));
        assert!(!names.contains(&"nested".to_string()));
    }

    #[test]
    fn hidden_dirs_skipped_by_default() {
        let tmp = TempDir::new("hidden");
        fs::create_dir(tmp.path.join("visible")).unwrap();
        fs::create_dir(tmp.path.join(".hidden")).unwrap();

        let runner = DirSearchRunner::spawn();
        runner.query(tmp.path.clone(), String::new(), false);
        let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
            matches!(ev, DirSearchEvent::Results { .. })
        })
        .unwrap();
        let DirSearchEvent::Results { results, .. } = evt;
        let names: Vec<String> = results
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"visible".to_string()));
        assert!(!names.iter().any(|n| n == ".hidden"));
    }

    #[test]
    fn node_modules_and_target_skipped() {
        let tmp = TempDir::new("heavy");
        fs::create_dir(tmp.path.join("src")).unwrap();
        fs::create_dir(tmp.path.join("node_modules")).unwrap();
        fs::create_dir(tmp.path.join("target")).unwrap();

        let runner = DirSearchRunner::spawn();
        runner.query(tmp.path.clone(), String::new(), false);
        let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
            matches!(ev, DirSearchEvent::Results { .. })
        })
        .unwrap();
        let DirSearchEvent::Results { results, .. } = evt;
        let names: Vec<String> = results
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|n| n == "src"));
        assert!(!names.iter().any(|n| n == "node_modules"));
        assert!(!names.iter().any(|n| n == "target"));
    }

    #[test]
    fn show_hidden_includes_dotfiles() {
        let tmp = TempDir::new("show-hidden");
        fs::create_dir(tmp.path.join("visible")).unwrap();
        fs::create_dir(tmp.path.join(".cache")).unwrap();

        let runner = DirSearchRunner::spawn();
        runner.query(tmp.path.clone(), String::new(), true);
        let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
            matches!(ev, DirSearchEvent::Results { .. })
        })
        .unwrap();
        let DirSearchEvent::Results { results, .. } = evt;
        let names: Vec<String> = results
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&".cache".to_string()));
    }

    #[test]
    fn cancel_supersedes_older_request() {
        let tmp = TempDir::new("cancel");
        fs::create_dir(tmp.path.join("alpha")).unwrap();

        let runner = DirSearchRunner::spawn();
        let id1 = runner.query(tmp.path.clone(), "alpha".into(), false);
        let id2 = runner.query(tmp.path.clone(), String::new(), false);
        assert!(id2 > id1);
        // Drain every event we receive within the deadline. The
        // supersession contract: anything still tagged id1 must NOT
        // arrive (its request was already invalidated by id2 by the
        // time the worker checked the active latch).
        let deadline = Duration::from_secs(2);
        let start = Instant::now();
        let mut saw_id2 = false;
        while start.elapsed() < deadline {
            match runner.events.recv_timeout(Duration::from_millis(50)) {
                Ok(DirSearchEvent::Results { request_id, .. }) => {
                    assert_ne!(request_id, id1, "id1 must not be delivered");
                    if request_id == id2 {
                        saw_id2 = true;
                        break;
                    }
                }
                Err(_) => continue,
            }
        }
        assert!(saw_id2, "id2's results must arrive");
    }

    #[test]
    fn missing_path_returns_empty_results() {
        let bogus = PathBuf::from("/this/path/does/not/exist/abc-xyz-nit");
        let runner = DirSearchRunner::spawn();
        runner.query(bogus.clone(), String::new(), false);
        let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
            matches!(ev, DirSearchEvent::Results { .. })
        })
        .expect("results");
        let DirSearchEvent::Results { results, .. } = evt;
        assert!(results.is_empty());
    }

    #[test]
    fn ranked_results_for_needle() {
        let tmp = TempDir::new("rank");
        fs::create_dir(tmp.path.join("alpha")).unwrap();
        fs::create_dir(tmp.path.join("alphabet")).unwrap();
        fs::create_dir(tmp.path.join("beta")).unwrap();

        let runner = DirSearchRunner::spawn();
        runner.query(tmp.path.clone(), "alp".into(), false);
        let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
            matches!(ev, DirSearchEvent::Results { .. })
        })
        .unwrap();
        let DirSearchEvent::Results { results, .. } = evt;
        let names: Vec<String> = results
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|n| n == "alpha"));
        assert!(names.iter().any(|n| n == "alphabet"));
        assert!(!names.iter().any(|n| n == "beta"));
    }
}
