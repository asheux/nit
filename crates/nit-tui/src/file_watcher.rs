//! Background file-watcher for workspace change detection.
//!
//! Monitors individual files and recursively scans workspace directories
//! for new or modified source files using mtime polling. Communicates
//! with the owning thread through typed channels — no platform-specific
//! filesystem notification APIs required.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

// ── Configuration ──────────────────────────────────────────────────

/// Polling cadence for checking file modification times.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Interval between workspace re-scans to discover newly created files.
const RESCAN_INTERVAL: Duration = Duration::from_secs(2);

/// Capacity hint for the initial mtime tracking map allocation.
const INITIAL_TRACKING_CAPACITY: usize = 256;

/// Directory names unconditionally excluded during recursive walks.
const IGNORED_DIRS: &[&str] = &["target", "node_modules", "__pycache__", "vendor", ".git"];

/// File extensions recognized as trackable source or config files.
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "c", "cpp", "h", "hpp", "go", "swift", "java", "kt", "scala", "cs", "py", "rb", "sh",
    "bash", "zsh", "js", "jsx", "ts", "tsx", "html", "css", "toml", "yaml", "yml", "json", "sql",
    "md", "txt",
];

// ── Per-file mtime tracking ────────────────────────────────────────

/// Recorded modification timestamp for a tracked file.
///
/// Wraps `Option<SystemTime>` so the comparison logic between baseline
/// and current mtime lives in one place rather than being scattered
/// across call-sites.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordedMtime(Option<SystemTime>);

impl RecordedMtime {
    /// Read the current mtime of `path`, returning a `None` interior
    /// on any I/O or metadata error.
    fn probe(path: &Path) -> Self {
        Self(std::fs::metadata(path).ok().and_then(|m| m.modified().ok()))
    }

    /// Whether this timestamp is present and valid.
    const fn is_known(&self) -> bool {
        self.0.is_some()
    }

    /// Returns `true` when the on-disk mtime has diverged from our record.
    fn has_changed_from(&self, baseline: &Self) -> bool {
        self.is_known() && *self != *baseline
    }
}

/// Format a recorded mtime as a human-readable diagnostic string.
impl fmt::Display for RecordedMtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(timestamp) => {
                let elapsed = timestamp
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default();
                write!(f, "mtime({}s)", elapsed.as_secs())
            }
            None => f.write_str("mtime(unknown)"),
        }
    }
}

/// Per-file modification-time tracking map.
type MtimeMap = HashMap<PathBuf, RecordedMtime>;

// ── Public command / event types ───────────────────────────────────

/// Commands sent from the main thread to the watcher thread.
pub enum FileWatchCommand {
    /// Begin tracking mtime changes for a single file.
    Watch(PathBuf),

    /// Remove a file from the tracked set.
    Unwatch(PathBuf),

    /// Recursively discover and track all source files under a root.
    WatchWorkspace(PathBuf),

    /// Gracefully terminate the watcher thread.
    Shutdown,
}

/// Render commands for diagnostic logging in the watcher subsystem.
impl fmt::Display for FileWatchCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Watch(target) => write!(f, "Watch({})", target.display()),
            Self::Unwatch(target) => write!(f, "Unwatch({})", target.display()),
            Self::WatchWorkspace(root) => write!(f, "WatchWorkspace({})", root.display()),
            Self::Shutdown => f.write_str("Shutdown"),
        }
    }
}

// ── Public handle ──────────────────────────────────────────────────

/// Main-thread handle for the background file-watcher.
///
/// Owns the command sender and join handle. Dropping without calling
/// [`FileWatcher::shutdown`] leaves the thread running until the
/// channel disconnects naturally.
pub struct FileWatcher {
    command_channel: Sender<FileWatchCommand>,

    /// Receiver for paths whose mtimes changed since the last drain.
    pub events: Receiver<PathBuf>,

    watcher_thread: Option<JoinHandle<()>>,
}

impl FileWatcher {
    /// Spawn the background polling thread and return a communication handle.
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (evt_tx, evt_rx) = mpsc::channel();

        let thread_handle = thread::Builder::new()
            .name("nit-file-watcher".into())
            .spawn(move || {
                PollLoop::initialize(cmd_rx, evt_tx).run_until_shutdown();
            })
            .expect("failed to spawn file-watcher thread");

        Self {
            command_channel: cmd_tx,
            events: evt_rx,
            watcher_thread: Some(thread_handle),
        }
    }

    /// Send a command to the watcher thread (best-effort, ignores closed channel).
    pub fn send(&self, command: FileWatchCommand) {
        let _ = self.command_channel.send(command);
    }

    /// Convenience: begin watching a single file.
    pub fn watch(&self, path: PathBuf) {
        self.send(FileWatchCommand::Watch(path));
    }

    /// Convenience: discover and watch all source files under `root`.
    pub fn watch_workspace(&self, root: PathBuf) {
        self.send(FileWatchCommand::WatchWorkspace(root));
    }

    /// Signal shutdown and block until the watcher thread exits.
    pub fn shutdown(&mut self) {
        let _ = self.command_channel.send(FileWatchCommand::Shutdown);
        if let Some(joinable) = self.watcher_thread.take() {
            let _ = joinable.join();
        }
    }
}

// ── Internal poll loop ─────────────────────────────────────────────

/// State machine driving the background polling loop.
///
/// Each tick: drain queued commands → optionally re-scan workspace →
/// compare tracked mtimes → emit change events → sleep until next tick.
struct PollLoop {
    tracked_files: MtimeMap,
    workspace_root: Option<PathBuf>,
    gitignored_dirs: Vec<String>,
    last_scan_timestamp: Instant,
    command_receiver: Receiver<FileWatchCommand>,
    change_emitter: Sender<PathBuf>,
}

impl PollLoop {
    /// Create a fresh poll-loop state from the given channel endpoints.
    fn initialize(command_rx: Receiver<FileWatchCommand>, event_tx: Sender<PathBuf>) -> Self {
        Self {
            tracked_files: HashMap::with_capacity(INITIAL_TRACKING_CAPACITY),
            workspace_root: None,
            gitignored_dirs: Vec::new(),
            last_scan_timestamp: Instant::now(),
            command_receiver: command_rx,
            change_emitter: event_tx,
        }
    }

    /// Execute polling ticks until a shutdown signal or channel disconnect.
    fn run_until_shutdown(&mut self) {
        loop {
            if self.drain_pending_commands() {
                return;
            }

            self.rescan_workspace_if_due();
            self.emit_detected_changes();

            if self.sleep_or_handle_command() {
                return;
            }
        }
    }

    // ── Command dispatch ──────────────────────────────────────────

    /// Apply a single command, returning `true` when the loop should exit.
    fn execute(&mut self, command: FileWatchCommand) -> bool {
        match command {
            FileWatchCommand::Watch(filepath) => {
                let baseline = RecordedMtime::probe(&filepath);
                self.tracked_files.insert(filepath, baseline);
                false
            }
            FileWatchCommand::Unwatch(filepath) => {
                self.tracked_files.remove(&filepath);
                false
            }
            FileWatchCommand::WatchWorkspace(root) => {
                self.gitignored_dirs = parse_gitignore_dirs(&root);
                let new_files =
                    discover_source_files(&root, &mut self.tracked_files, &self.gitignored_dirs);
                for path in new_files {
                    let _ = self.change_emitter.send(path);
                }
                self.workspace_root = Some(root);
                self.last_scan_timestamp = Instant::now();
                false
            }
            FileWatchCommand::Shutdown => true,
        }
    }

    /// Non-blocking drain of every pending command. Returns `true` on shutdown.
    fn drain_pending_commands(&mut self) -> bool {
        loop {
            match self.command_receiver.try_recv() {
                Ok(cmd) => {
                    if self.execute(cmd) {
                        return true;
                    }
                }
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => return true,
            }
        }
    }

    // ── Workspace scanning ────────────────────────────────────────

    /// Re-discover workspace source files when the rescan interval elapses.
    fn rescan_workspace_if_due(&mut self) {
        let workspace = match self.workspace_root {
            Some(ref root) => root.clone(),
            None => return,
        };

        if self.last_scan_timestamp.elapsed() < RESCAN_INTERVAL {
            return;
        }

        let new_files =
            discover_source_files(&workspace, &mut self.tracked_files, &self.gitignored_dirs);
        // Emit change events for newly created files so the genome system
        // picks them up immediately rather than waiting for a future mtime change.
        for path in new_files {
            let _ = self.change_emitter.send(path);
        }
        self.last_scan_timestamp = Instant::now();
    }

    // ── Change detection ──────────────────────────────────────────

    /// Compare on-disk mtimes against recorded values and emit change events.
    fn emit_detected_changes(&mut self) {
        let modified_paths: Vec<PathBuf> = self
            .tracked_files
            .iter_mut()
            .filter_map(|(filepath, recorded)| {
                let current = RecordedMtime::probe(filepath);
                if !current.has_changed_from(recorded) {
                    return None;
                }
                *recorded = current;
                Some(filepath.clone())
            })
            .collect();

        for changed_file in modified_paths {
            let _ = self.change_emitter.send(changed_file);
        }
    }

    /// Block up to [`POLL_INTERVAL`] waiting for a command, handling it
    /// inline. Returns `true` when the loop should exit.
    fn sleep_or_handle_command(&mut self) -> bool {
        match self.command_receiver.recv_timeout(POLL_INTERVAL) {
            Ok(incoming) => self.execute(incoming),
            Err(mpsc::RecvTimeoutError::Timeout) => false,
            Err(mpsc::RecvTimeoutError::Disconnected) => true,
        }
    }
}

// ── Source-file discovery ──────────────────────────────────────────

/// Walk `root` recursively and insert newly discovered source files
/// into `destination`. Paths already present are left unchanged so
/// their baseline mtime records are preserved.
/// Returns the list of newly discovered paths (not previously tracked).
fn discover_source_files(
    root: &Path,
    destination: &mut MtimeMap,
    gitignored: &[String],
) -> Vec<PathBuf> {
    let mut new_paths = Vec::new();
    SourceTreeWalker::rooted_at(root, gitignored.to_vec()).for_each(|discovered_path| {
        use std::collections::hash_map::Entry;
        match destination.entry(discovered_path.clone()) {
            Entry::Vacant(e) => {
                e.insert(RecordedMtime::probe(&discovered_path));
                new_paths.push(discovered_path);
            }
            Entry::Occupied(_) => {} // already tracked
        }
    });
    new_paths
}

// ── Extension classification ──────────────────────────────────────

/// Returns `true` when the extension belongs to a recognized source
/// or configuration format listed in [`SOURCE_EXTENSIONS`].
fn matches_known_extension(candidate: &str) -> bool {
    SOURCE_EXTENSIONS.contains(&candidate)
}

/// Check whether `filepath` carries a recognized source extension.
fn is_trackable_source(filepath: &Path) -> bool {
    filepath
        .extension()
        .and_then(|raw| raw.to_str())
        .is_some_and(matches_known_extension)
}

// ── Directory filtering ───────────────────────────────────────────

/// Returns `true` for hidden directories (leading `.`) and well-known
/// build output directories listed in [`IGNORED_DIRS`].
fn is_excluded_directory(dir_component: &str, gitignored: &[String]) -> bool {
    dir_component.starts_with('.')
        || IGNORED_DIRS.contains(&dir_component)
        || gitignored.iter().any(|g| g == dir_component)
}

/// Parse `.gitignore` at the workspace root and collect directory names to skip.
/// Public so it can be called at startup to populate `AppState.gitignored_dirs`.
/// Only extracts simple directory patterns (e.g., `build/`, `dist`, `/out`).
/// Does not support globs, negations, or nested `.gitignore` files.
pub fn parse_gitignore_dirs(workspace_root: &Path) -> Vec<String> {
    let gitignore_path = workspace_root.join(".gitignore");
    let content = match std::fs::read_to_string(&gitignore_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            // Skip comments and empty lines.
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            // Skip negations.
            if line.starts_with('!') {
                return None;
            }
            // Skip glob patterns (contain * or ?).
            if line.contains('*') || line.contains('?') {
                return None;
            }
            // Strip leading `/` (root-anchored).
            let line = line.strip_prefix('/').unwrap_or(line);
            // Strip trailing `/` (directory marker).
            let line = line.strip_suffix('/').unwrap_or(line);
            // Only take simple names (no path separators inside).
            if line.contains('/') || line.is_empty() {
                return None;
            }
            Some(line.to_string())
        })
        .collect()
}

// ── Filesystem entry classification ───────────────────────────────

/// Classification of a filesystem entry encountered during a walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    /// A traversable directory whose children should be expanded.
    Descendable,

    /// A file matching a recognized source extension.
    Trackable,

    /// An entry that does not match any watched criteria.
    Irrelevant,
}

impl EntryKind {
    /// Classify a path for the directory walker, inspecting the filesystem
    /// to distinguish directories from files.
    fn classify(entry_path: &Path) -> Self {
        if entry_path.is_dir() {
            return Self::Descendable;
        }
        if is_trackable_source(entry_path) {
            return Self::Trackable;
        }
        Self::Irrelevant
    }
}

/// Render the entry classification as a short label for diagnostics.
impl fmt::Display for EntryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Descendable => "directory",
            Self::Trackable => "source",
            Self::Irrelevant => "skip",
        };
        f.write_str(label)
    }
}

// ── Directory walker ──────────────────────────────────────────────

/// Stack-based depth-first iterator yielding source-file paths.
///
/// Automatically skips hidden directories, common build artifacts, and
/// non-source files while recursively walking the workspace tree.
struct SourceTreeWalker {
    pending_paths: Vec<PathBuf>,
    gitignored: Vec<String>,
}

impl SourceTreeWalker {
    /// Begin a walk rooted at the given directory.
    fn rooted_at(start: &Path, gitignored: Vec<String>) -> Self {
        Self {
            pending_paths: vec![start.to_path_buf()],
            gitignored,
        }
    }

    /// Push walkable children of `parent` onto the exploration stack,
    /// respecting the directory exclusion list and .gitignore.
    fn expand_directory(&mut self, parent: &Path) {
        let dir_name = parent
            .file_name()
            .and_then(|segment| segment.to_str())
            .unwrap_or("");

        if is_excluded_directory(dir_name, &self.gitignored) {
            return;
        }

        let Ok(listing) = std::fs::read_dir(parent) else {
            return;
        };

        for child in listing.flatten() {
            self.pending_paths.push(child.path());
        }
    }
}

/// Depth-first traversal yielding only paths with recognized source extensions.
impl Iterator for SourceTreeWalker {
    type Item = PathBuf;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(candidate) = self.pending_paths.pop() {
            match EntryKind::classify(&candidate) {
                EntryKind::Descendable => self.expand_directory(&candidate),
                EntryKind::Trackable => return Some(candidate),
                EntryKind::Irrelevant => {}
            }
        }
        None
    }
}
