//! Lightweight background file watcher that monitors the workspace for
//! external changes (e.g. agent writes) using mtime polling.
//!
//! Watches individual files and scans the workspace for new or modified
//! source files. The watcher runs on a dedicated thread and communicates
//! via channels — no filesystem notification APIs required.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

/// Directory names unconditionally skipped during workspace scanning.
const IGNORED_DIRS: &[&str] = &["target", "node_modules", "__pycache__", "vendor", ".git"];

/// Per-file modification-time tracking map.
type MtimeMap = HashMap<PathBuf, Option<SystemTime>>;

/// Polling cadence for checking file changes.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// How often the watcher re-discovers new source files in the workspace.
const RESCAN_INTERVAL: Duration = Duration::from_secs(2);

/// Commands sent to the watcher thread from the main thread.
pub enum FileWatchCommand {
    /// Begin tracking mtime changes for a single file.
    Watch(PathBuf),
    /// Remove a file from the tracked set.
    Unwatch(PathBuf),
    /// Recursively discover and track all source files under a root directory.
    WatchWorkspace(PathBuf),
    /// Gracefully terminate the watcher thread.
    Shutdown,
}

/// Handle held by the main thread for communicating with the background
/// file-watcher. Dropping the handle without calling [`FileWatcher::shutdown`]
/// will leave the thread running until the channel disconnects.
pub struct FileWatcher {
    cmd_tx: Sender<FileWatchCommand>,
    /// Channel of paths that changed on disk since last drain.
    pub events: Receiver<PathBuf>,
    join_handle: Option<JoinHandle<()>>,
}

impl FileWatcher {
    /// Spawn a background polling thread and return a handle for
    /// sending commands and receiving change events.
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        let join_handle = thread::Builder::new()
            .name("nit-file-watcher".into())
            .spawn(move || {
                WatcherState::new(cmd_rx, event_tx).run();
            })
            .expect("spawn file watcher");

        Self {
            cmd_tx,
            events: event_rx,
            join_handle: Some(join_handle),
        }
    }

    /// Send an arbitrary command to the watcher thread.
    pub fn send(&self, command: FileWatchCommand) {
        let _ = self.cmd_tx.send(command);
    }

    /// Convenience: begin watching a single file path.
    pub fn watch(&self, path: PathBuf) {
        self.send(FileWatchCommand::Watch(path));
    }

    /// Convenience: discover and watch all source files under `root`.
    pub fn watch_workspace(&self, root: PathBuf) {
        self.send(FileWatchCommand::WatchWorkspace(root));
    }

    /// Signal shutdown and block until the watcher thread has joined.
    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(FileWatchCommand::Shutdown);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Internal state of the background polling loop.
///
/// The watcher alternates between draining queued commands, re-scanning
/// the workspace (if one is set), emitting change events, and sleeping
/// until the next poll tick.
struct WatcherState {
    tracked_mtimes: MtimeMap,
    workspace_root: Option<PathBuf>,
    last_scan_at: Instant,
    cmd_rx: Receiver<FileWatchCommand>,
    change_tx: Sender<PathBuf>,
}

impl WatcherState {
    fn new(cmd_rx: Receiver<FileWatchCommand>, change_tx: Sender<PathBuf>) -> Self {
        Self {
            tracked_mtimes: HashMap::new(),
            workspace_root: None,
            last_scan_at: Instant::now(),
            cmd_rx,
            change_tx,
        }
    }

    /// Run the poll loop until shutdown or channel disconnect.
    fn run(&mut self) {
        while self.poll_cycle() {}
    }

    /// Execute one polling cycle. Returns `true` to continue, `false` to stop.
    fn poll_cycle(&mut self) -> bool {
        if self.drain_pending_commands() {
            return false;
        }
        self.rescan_workspace_if_due();
        self.emit_changed_files();
        !self.wait_for_next_command()
    }

    /// Dispatch a single command. Returns `true` when the watcher should stop.
    fn apply_command(&mut self, command: FileWatchCommand) -> bool {
        match command {
            FileWatchCommand::Watch(watched_path) => {
                let mtime = file_mtime(&watched_path);
                self.tracked_mtimes.insert(watched_path, mtime);
            }
            FileWatchCommand::Unwatch(unwatched_path) => {
                self.tracked_mtimes.remove(&unwatched_path);
            }
            FileWatchCommand::WatchWorkspace(root_dir) => {
                discover_source_files(&root_dir, &mut self.tracked_mtimes);
                self.workspace_root = Some(root_dir);
                self.last_scan_at = Instant::now();
            }
            FileWatchCommand::Shutdown => return true,
        }
        false
    }

    /// Non-blocking: drain every queued command. Returns `true` on shutdown.
    fn drain_pending_commands(&mut self) -> bool {
        loop {
            let command = match self.cmd_rx.try_recv() {
                Ok(received_cmd) => received_cmd,
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => return true,
            };

            if self.apply_command(command) {
                return true;
            }
        }
    }

    /// If a workspace root is configured and the rescan interval has elapsed,
    /// re-discover source files so newly created files get picked up.
    fn rescan_workspace_if_due(&mut self) {
        let root_path = match self.workspace_root {
            Some(ref configured_root) => configured_root.clone(),
            None => return,
        };

        if self.last_scan_at.elapsed() < RESCAN_INTERVAL {
            return;
        }

        discover_source_files(&root_path, &mut self.tracked_mtimes);
        self.last_scan_at = Instant::now();
    }

    /// Compare current mtimes against recorded values and send change events.
    fn emit_changed_files(&mut self) {
        let changed_paths: Vec<PathBuf> = self
            .tracked_mtimes
            .iter_mut()
            .filter_map(|(filepath, recorded_mtime)| {
                let current_mtime = file_mtime(filepath);
                let is_unchanged = current_mtime == *recorded_mtime || current_mtime.is_none();
                if is_unchanged {
                    return None;
                }
                *recorded_mtime = current_mtime;
                Some(filepath.clone())
            })
            .collect();

        for changed_path in changed_paths {
            let _ = self.change_tx.send(changed_path);
        }
    }

    /// Block up to [`POLL_INTERVAL`] for the next command. Returns `true` on shutdown.
    fn wait_for_next_command(&mut self) -> bool {
        match self.cmd_rx.recv_timeout(POLL_INTERVAL) {
            Ok(received_cmd) => self.apply_command(received_cmd),
            Err(mpsc::RecvTimeoutError::Timeout) => false,
            Err(mpsc::RecvTimeoutError::Disconnected) => true,
        }
    }
}

/// Walk `root` recursively, inserting newly discovered source-file
/// paths into the mtime tracking map. Existing entries are preserved.
fn discover_source_files(root: &Path, tracked_mtimes: &mut MtimeMap) {
    for path in SourceFileWalker::from_root(root) {
        tracked_mtimes
            .entry(path)
            .or_insert_with_key(|p| file_mtime(p));
    }
}

/// Read the modification time of a file, returning `None` on any error.
fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Check whether `ext` belongs to a recognised source language or config format.
fn is_source_extension(ext: &str) -> bool {
    matches!(
        ext,
        // Systems languages
        "rs" | "c" | "cpp" | "h" | "hpp" | "go" | "swift"
        // JVM and .NET
        | "java" | "kt" | "scala" | "cs"
        // Scripting and dynamic
        | "py" | "rb" | "sh" | "bash" | "zsh"
        // Web frontend
        | "js" | "jsx" | "ts" | "tsx" | "html" | "css"
        // Data and config
        | "toml" | "yaml" | "yml" | "json" | "sql"
        // Documentation
        | "md" | "txt"
    )
}

/// Check whether `path` has a recognised source-file extension.
fn is_source_file(path: &Path) -> bool {
    path.extension()
        .and_then(|os_ext| os_ext.to_str())
        .is_some_and(is_source_extension)
}

/// Returns `true` for hidden directories (leading `.`) and well-known
/// build artifact directories that should be skipped during walking.
fn is_ignored_directory(directory_name: &str) -> bool {
    directory_name.starts_with('.') || IGNORED_DIRS.contains(&directory_name)
}

/// Classification of a filesystem entry encountered during directory walking.
enum EntryKind {
    /// A directory that should be expanded into its children.
    Directory,
    /// A file with a recognised source extension.
    SourceFile,
    /// A file or symlink that does not match any watched extension.
    Ignored,
}

/// Classify a filesystem path as a directory, source file, or ignored entry.
fn classify_entry(path: &Path) -> EntryKind {
    if path.is_dir() {
        EntryKind::Directory
    } else if is_source_file(path) {
        EntryKind::SourceFile
    } else {
        EntryKind::Ignored
    }
}

/// Stack-based depth-first iterator yielding source-file paths.
///
/// Automatically skips hidden directories, common build artifacts, and
/// non-source files, providing a filtered view of the workspace tree.
struct SourceFileWalker {
    pending_entries: Vec<PathBuf>,
}

impl SourceFileWalker {
    /// Create a walker rooted at the given directory.
    fn from_root(root: &Path) -> Self {
        Self {
            pending_entries: vec![root.to_path_buf()],
        }
    }

    /// Push child entries of `dir_path` onto the stack, skipping ignored directories.
    fn expand_directory(&mut self, dir_path: &Path) {
        let dir_name = dir_path
            .file_name()
            .and_then(|os_name| os_name.to_str())
            .unwrap_or("");

        if is_ignored_directory(dir_name) {
            return;
        }

        let readable_entries = match std::fs::read_dir(dir_path) {
            Ok(iter) => iter,
            Err(_) => return,
        };

        for entry_result in readable_entries.flatten() {
            self.pending_entries.push(entry_result.path());
        }
    }
}

impl Iterator for SourceFileWalker {
    type Item = PathBuf;

    fn next(&mut self) -> Option<PathBuf> {
        loop {
            let candidate = self.pending_entries.pop()?;
            match classify_entry(&candidate) {
                EntryKind::Directory => self.expand_directory(&candidate),
                EntryKind::SourceFile => return Some(candidate),
                EntryKind::Ignored => {}
            }
        }
    }
}
