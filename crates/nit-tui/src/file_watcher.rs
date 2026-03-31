//! Background file-watcher for workspace change detection.
//!
//! Monitors individual files and recursively scans workspace directories
//! for new or modified source files using mtime polling. Communicates
//! with the owning thread through typed channels — no platform-specific
//! filesystem notification APIs required.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

// ── Configuration ──────────────────────────────────────────────────

/// Polling cadence for checking file modification times.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Interval between workspace re-scans to discover newly created files.
const RESCAN_INTERVAL: Duration = Duration::from_secs(2);

/// Directory names unconditionally excluded during recursive walks.
const IGNORED_DIRS: &[&str] = &["target", "node_modules", "__pycache__", "vendor", ".git"];

/// File extensions recognized as trackable source or config files.
const SOURCE_EXTENSIONS: &[&str] = &[
    // Systems
    "rs", "c", "cpp", "h", "hpp", "go", "swift",
    // JVM / .NET
    "java", "kt", "scala", "cs",
    // Scripting
    "py", "rb", "sh", "bash", "zsh",
    // Web
    "js", "jsx", "ts", "tsx", "html", "css",
    // Data / config
    "toml", "yaml", "yml", "json", "sql",
    // Prose
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

// ── Public handle ──────────────────────────────────────────────────

/// Main-thread handle for the background file-watcher.
///
/// Owns the command sender and join handle. Dropping without calling
/// [`FileWatcher::shutdown`] leaves the thread running until the
/// channel disconnects naturally.
pub struct FileWatcher {
    command_sender: Sender<FileWatchCommand>,

    /// Receiver for paths whose mtimes changed since the last drain.
    pub events: Receiver<PathBuf>,

    watcher_thread: Option<JoinHandle<()>>,
}

impl FileWatcher {
    /// Spawn the background polling thread and return a communication handle.
    pub fn spawn() -> Self {
        let (command_sender, command_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();

        let watcher_thread = thread::Builder::new()
            .name("nit-file-watcher".into())
            .spawn(move || {
                PollLoop::initialize(command_receiver, event_sender).run_until_shutdown();
            })
            .expect("failed to spawn file-watcher thread");

        Self {
            command_sender,
            events: event_receiver,
            watcher_thread: Some(watcher_thread),
        }
    }

    /// Send a command to the watcher thread (best-effort, ignores closed channel).
    pub fn send(&self, command: FileWatchCommand) {
        let _ = self.command_sender.send(command);
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
        let _ = self.command_sender.send(FileWatchCommand::Shutdown);
        if let Some(thread_handle) = self.watcher_thread.take() {
            let _ = thread_handle.join();
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
    last_workspace_scan: Instant,
    command_receiver: Receiver<FileWatchCommand>,
    change_emitter: Sender<PathBuf>,
}

impl PollLoop {
    /// Create a fresh poll-loop state from the given channel endpoints.
    fn initialize(
        command_receiver: Receiver<FileWatchCommand>,
        change_emitter: Sender<PathBuf>,
    ) -> Self {
        Self {
            tracked_files: HashMap::with_capacity(256),
            workspace_root: None,
            last_workspace_scan: Instant::now(),
            command_receiver,
            change_emitter,
        }
    }

    /// Execute polling ticks until a shutdown signal or channel disconnect.
    fn run_until_shutdown(&mut self) {
        loop {
            if self.drain_queued_commands() {
                break;
            }

            self.rescan_workspace_if_due();
            self.emit_mtime_changes();

            if self.block_until_next_tick() {
                break;
            }
        }
    }

    // ── Command handling ───────────────────────────────────────────

    /// Apply a single command. Returns `true` when the loop should exit.
    fn execute_command(&mut self, command: FileWatchCommand) -> bool {
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
            FileWatchCommand::WatchWorkspace(root_directory) => {
                populate_source_entries(&root_directory, &mut self.tracked_files);
                self.workspace_root = Some(root_directory);
                self.last_workspace_scan = Instant::now();
                false
            }
            FileWatchCommand::Shutdown => true,
        }
    }

    /// Non-blocking drain of every pending command. Returns `true` on shutdown.
    fn drain_queued_commands(&mut self) -> bool {
        loop {
            match self.command_receiver.try_recv() {
                Ok(pending_command) => {
                    if self.execute_command(pending_command) {
                        return true;
                    }
                }
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => return true,
            }
        }
    }

    // ── Workspace scanning ─────────────────────────────────────────

    /// Re-discover workspace source files when the rescan interval elapses.
    fn rescan_workspace_if_due(&mut self) {
        let workspace = match self.workspace_root {
            Some(ref active_root) => active_root.clone(),
            None => return,
        };

        if self.last_workspace_scan.elapsed() < RESCAN_INTERVAL {
            return;
        }

        populate_source_entries(&workspace, &mut self.tracked_files);
        self.last_workspace_scan = Instant::now();
    }

    // ── Change detection ───────────────────────────────────────────

    /// Compare on-disk mtimes against recorded values and emit change events.
    fn emit_mtime_changes(&mut self) {
        let modified_paths: Vec<PathBuf> = self
            .tracked_files
            .iter_mut()
            .filter_map(|(filepath, recorded_mtime)| {
                let current_mtime = RecordedMtime::probe(filepath);

                let unchanged =
                    current_mtime.0.is_none() || current_mtime == *recorded_mtime;
                if unchanged {
                    return None;
                }

                *recorded_mtime = current_mtime;
                Some(filepath.clone())
            })
            .collect();

        for modified_file in modified_paths {
            let _ = self.change_emitter.send(modified_file);
        }
    }

    /// Block up to [`POLL_INTERVAL`] waiting for a command.
    /// Returns `true` when the loop should exit.
    fn block_until_next_tick(&mut self) -> bool {
        match self.command_receiver.recv_timeout(POLL_INTERVAL) {
            Ok(incoming_command) => self.execute_command(incoming_command),
            Err(mpsc::RecvTimeoutError::Timeout) => false,
            Err(mpsc::RecvTimeoutError::Disconnected) => true,
        }
    }
}

// ── Source-file discovery ──────────────────────────────────────────

/// Walk `root` recursively and insert newly discovered source files
/// into `destination`. Paths already present are left unchanged so
/// their baseline mtime records are preserved.
fn populate_source_entries(root: &Path, destination: &mut MtimeMap) {
    for discovered_path in DirectoryWalker::rooted_at(root) {
        destination
            .entry(discovered_path)
            .or_insert_with_key(|p| RecordedMtime::probe(p));
    }
}

// ── Extension classification ───────────────────────────────────────

/// Returns `true` when the extension belongs to a recognized source
/// or configuration format listed in [`SOURCE_EXTENSIONS`].
fn matches_source_extension(candidate_ext: &str) -> bool {
    SOURCE_EXTENSIONS.contains(&candidate_ext)
}

/// Check whether `filepath` carries a recognized source extension.
fn has_source_extension(filepath: &Path) -> bool {
    filepath
        .extension()
        .and_then(|raw_ext| raw_ext.to_str())
        .is_some_and(matches_source_extension)
}

// ── Directory filtering ────────────────────────────────────────────

/// Returns `true` for hidden directories (leading `.`) and well-known
/// build output directories listed in [`IGNORED_DIRS`].
fn should_skip_directory(dir_label: &str) -> bool {
    dir_label.starts_with('.') || IGNORED_DIRS.contains(&dir_label)
}

// ── Filesystem entry classification ────────────────────────────────

/// Classification of a filesystem entry encountered during a walk.
enum WalkerEntry {
    /// A traversable directory whose children should be expanded.
    Subdirectory,

    /// A file matching a recognized source extension.
    TrackableFile,

    /// An entry that does not match any watched criteria.
    Skippable,
}

/// Classify a path for the directory walker.
fn categorize_path(entry_path: &Path) -> WalkerEntry {
    if entry_path.is_dir() {
        return WalkerEntry::Subdirectory;
    }

    if has_source_extension(entry_path) {
        return WalkerEntry::TrackableFile;
    }

    WalkerEntry::Skippable
}

// ── Directory walker ───────────────────────────────────────────────

/// Stack-based depth-first iterator yielding source-file paths.
///
/// Automatically skips hidden directories, common build artifacts, and
/// non-source files while recursively walking the workspace tree.
struct DirectoryWalker {
    exploration_stack: Vec<PathBuf>,
}

impl DirectoryWalker {
    /// Begin a walk rooted at the given directory.
    fn rooted_at(start: &Path) -> Self {
        Self {
            exploration_stack: vec![start.to_path_buf()],
        }
    }

    /// Push walkable children of `parent` onto the exploration stack,
    /// skipping directories that match the ignore list.
    fn enqueue_children(&mut self, parent_directory: &Path) {
        let dir_label = parent_directory
            .file_name()
            .and_then(|segment| segment.to_str())
            .unwrap_or("");

        if should_skip_directory(dir_label) {
            return;
        }

        let children = match std::fs::read_dir(parent_directory) {
            Ok(listing) => listing,
            Err(_) => return,
        };

        for child_entry in children.flatten() {
            self.exploration_stack.push(child_entry.path());
        }
    }
}

impl Iterator for DirectoryWalker {
    type Item = PathBuf;

    fn next(&mut self) -> Option<PathBuf> {
        while let Some(candidate_path) = self.exploration_stack.pop() {
            match categorize_path(&candidate_path) {
                WalkerEntry::Subdirectory => self.enqueue_children(&candidate_path),
                WalkerEntry::TrackableFile => return Some(candidate_path),
                WalkerEntry::Skippable => continue,
            }
        }
        None
    }
}
