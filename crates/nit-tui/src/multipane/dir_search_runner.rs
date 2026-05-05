//! Async directory walker that powers the per-pane dir-search overlay.
//!
//! Mirrors the shape of [`crate::fuzzy_search_runner::FuzzyMatcherRunner`]:
//! a single named worker thread, crossbeam channels for commands and
//! events, and an `Arc<AtomicU64>` request-id latch so a fresh
//! keystroke supersedes any in-flight walk. Two walk modes:
//!
//! - **Browse mode** (`needle` empty): list children of `base` and, for
//!   each child whose path is in the operator-supplied `expanded` set,
//!   inline its children one level deep so the renderer can show an
//!   in-place tree.
//! - **Search mode** (`needle` non-empty): bounded BFS up to
//!   `MAX_DEPTH` directories deep, scoring each candidate's path
//!   (relative to `base`, joined with `/`) against the needle via
//!   [`super::dir_search::rank`].
//!
//! Hidden files (anything starting with `.`), the heavyweight build
//! dirs (`node_modules`, `target`, `.venv`, `dist`, `build`) and any
//! bare-name `.gitignore` entries are filtered at the walker source.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{unbounded, Receiver, Sender};

use super::dir_search::rank;

const MAX_RESULTS: usize = 50;
const MAX_DEPTH: usize = 6;
const MAX_VISITED: usize = 4000;
const HEAVY_DENY: &[&str] = &["node_modules", "target", ".venv", "dist", "build"];

#[derive(Debug)]
pub enum DirSearchCommand {
    Query {
        request_id: u64,
        base: PathBuf,
        needle: String,
        show_hidden: bool,
        gitignored: Vec<String>,
        expanded: HashSet<PathBuf>,
    },
    Shutdown,
}

/// Single source of truth for "skip this directory while walking":
/// - `node_modules`/`target`/`.venv`/`dist`/`build` (hard-coded heavy
///   build dirs), unless the operator flips `show_hidden`
/// - bare-name `.gitignore` entries hydrated from the workspace root
/// - dotfiles, unless `show_hidden` is set
#[derive(Clone, Debug)]
struct WalkFilter {
    gitignored: Vec<String>,
    show_hidden: bool,
}

impl WalkFilter {
    fn excludes(&self, name: &str) -> bool {
        if !self.show_hidden && name.starts_with('.') {
            return true;
        }
        if HEAVY_DENY.contains(&name) {
            return true;
        }
        self.gitignored.iter().any(|g| g == name)
    }
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
    pub fn query(
        &self,
        base: PathBuf,
        needle: String,
        show_hidden: bool,
        gitignored: Vec<String>,
    ) -> u64 {
        self.query_with_expanded(base, needle, show_hidden, gitignored, HashSet::new())
    }

    pub fn query_with_expanded(
        &self,
        base: PathBuf,
        needle: String,
        show_hidden: bool,
        gitignored: Vec<String>,
        expanded: HashSet<PathBuf>,
    ) -> u64 {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.active_request.store(id, Ordering::Relaxed);
        let _ = self.cmd_tx.send(DirSearchCommand::Query {
            request_id: id,
            base,
            needle,
            show_hidden,
            gitignored,
            expanded,
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
                gitignored,
                expanded,
            } => {
                if is_request_stale(&active, request_id) {
                    continue;
                }
                let filter = WalkFilter {
                    gitignored,
                    show_hidden,
                };
                let results = walk_tree(&base, &needle, &filter, &expanded, &active, request_id);
                if is_request_stale(&active, request_id) {
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

fn walk_tree(
    base: &Path,
    needle: &str,
    filter: &WalkFilter,
    expanded: &HashSet<PathBuf>,
    active: &AtomicU64,
    request_id: u64,
) -> Vec<PathBuf> {
    if needle.is_empty() {
        walk_browse(base, filter, expanded, active, request_id)
    } else {
        walk_search(base, needle, filter, active, request_id)
    }
}

/// Browse mode: list direct children of `base` alphabetically, then
/// for each child whose path is in `expanded`, inline one level of its
/// children (and theirs, if also expanded — recursing through
/// `expanded` only). Output preserves a stable DFS order so the
/// renderer can indent by depth.
struct BrowseCtx<'a> {
    filter: &'a WalkFilter,
    expanded: &'a HashSet<PathBuf>,
    active: &'a AtomicU64,
    request_id: u64,
}

struct BrowseAcc {
    out: Vec<PathBuf>,
    visited: usize,
}

fn walk_browse(
    base: &Path,
    filter: &WalkFilter,
    expanded: &HashSet<PathBuf>,
    active: &AtomicU64,
    request_id: u64,
) -> Vec<PathBuf> {
    let ctx = BrowseCtx {
        filter,
        expanded,
        active,
        request_id,
    };
    let mut acc = BrowseAcc {
        out: Vec::new(),
        visited: 0,
    };
    walk_browse_into(base, 0, &ctx, &mut acc);
    if acc.out.len() > MAX_RESULTS {
        acc.out.truncate(MAX_RESULTS);
    }
    acc.out
}

fn walk_browse_into(dir: &Path, depth: usize, ctx: &BrowseCtx<'_>, acc: &mut BrowseAcc) {
    if is_request_stale(ctx.active, ctx.request_id) {
        return;
    }
    if depth >= MAX_DEPTH || acc.visited >= MAX_VISITED || acc.out.len() >= MAX_RESULTS {
        return;
    }
    let mut children = read_child_dirs(dir, ctx.filter);
    children.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    for child in children {
        acc.visited += 1;
        if acc.visited > MAX_VISITED || acc.out.len() >= MAX_RESULTS {
            return;
        }
        if is_request_stale(ctx.active, ctx.request_id) {
            return;
        }
        let recurse = ctx.expanded.contains(&child);
        acc.out.push(child.clone());
        if recurse {
            walk_browse_into(&child, depth + 1, ctx, acc);
        }
    }
}

/// Search mode: BFS up to `MAX_DEPTH` levels under `base`. Score each
/// directory's relative path (joined with `/`) against the needle and
/// keep the top `MAX_RESULTS` by score.
fn walk_search(
    base: &Path,
    needle: &str,
    filter: &WalkFilter,
    active: &AtomicU64,
    request_id: u64,
) -> Vec<PathBuf> {
    let mut scored: Vec<(i64, PathBuf)> = Vec::new();
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((base.to_path_buf(), 0));
    let mut visited = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        if is_request_stale(active, request_id) {
            return Vec::new();
        }
        if visited >= MAX_VISITED {
            break;
        }
        visited += 1;
        let children = read_child_dirs(&dir, filter);
        for child in children {
            if is_request_stale(active, request_id) {
                return Vec::new();
            }
            let haystack = relative_haystack(base, &child);
            if let Some(score) = rank(&haystack, needle) {
                scored.push((score, child.clone()));
            }
            if depth + 1 < MAX_DEPTH {
                queue.push_back((child, depth + 1));
            }
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.truncate(MAX_RESULTS);
    scored.into_iter().map(|(_, p)| p).collect()
}

fn read_child_dirs(dir: &Path, filter: &WalkFilter) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // symlink_metadata-style: only follow real directories so a
        // symlink loop can't blow past MAX_DEPTH × MAX_VISITED.
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if filter.excludes(&name_str) {
            continue;
        }
        out.push(entry.path());
    }
    out
}

fn relative_haystack(base: &Path, path: &Path) -> String {
    let Ok(rel) = path.strip_prefix(base) else {
        return path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
    };
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

fn is_request_stale(active: &AtomicU64, request_id: u64) -> bool {
    active.load(Ordering::Relaxed) != request_id
}

#[cfg(test)]
#[path = "../tests/multipane_dir_search_runner.rs"]
mod tests;
