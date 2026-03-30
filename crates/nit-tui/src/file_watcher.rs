//! Lightweight background file watcher that monitors the workspace for
//! external changes (e.g. agent writes) using mtime polling.
//! Watches individual files AND scans the workspace for new/modified source files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

/// Source file extensions to scan for in workspace mode.
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "jsx", "ts", "tsx", "go", "java", "c", "cpp", "h", "hpp", "cs", "rb",
    "swift", "kt", "scala", "sh", "bash", "zsh", "toml", "yaml", "yml", "json", "html", "css",
    "sql", "md", "txt",
];

/// Commands sent to the watcher thread.
pub enum FileWatchCommand {
    /// Start watching a file.
    Watch(PathBuf),
    /// Stop watching a file.
    Unwatch(PathBuf),
    /// Scan workspace root for all source files and watch them.
    WatchWorkspace(PathBuf),
    /// Shut down the watcher thread.
    Shutdown,
}

/// The watcher handle held by the main thread.
pub struct FileWatcher {
    cmd_tx: Sender<FileWatchCommand>,
    /// Paths that changed on disk since last drain.
    pub events: Receiver<PathBuf>,
    handle: Option<JoinHandle<()>>,
}

impl FileWatcher {
    /// Spawn the watcher background thread.
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("nit-file-watcher".into())
            .spawn(move || watcher_loop(cmd_rx, event_tx))
            .expect("spawn file watcher");
        Self {
            cmd_tx,
            events: event_rx,
            handle: Some(handle),
        }
    }

    pub fn send(&self, command: FileWatchCommand) {
        let _ = self.cmd_tx.send(command);
    }

    pub fn watch(&self, path: PathBuf) {
        self.send(FileWatchCommand::Watch(path));
    }

    pub fn watch_workspace(&self, root: PathBuf) {
        self.send(FileWatchCommand::WatchWorkspace(root));
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(FileWatchCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Background loop: polls watched files every ~200ms and rescans workspace periodically.
fn watcher_loop(cmd_rx: Receiver<FileWatchCommand>, event_tx: Sender<PathBuf>) {
    let mut watched: HashMap<PathBuf, Option<SystemTime>> = HashMap::new();
    let mut workspace_root: Option<PathBuf> = None;
    let mut last_workspace_scan = std::time::Instant::now();
    let poll_interval = Duration::from_millis(200);
    let workspace_scan_interval = Duration::from_secs(2);

    loop {
        // Drain all pending commands.
        loop {
            match cmd_rx.try_recv() {
                Ok(FileWatchCommand::Watch(path)) => {
                    let mtime = file_mtime(&path);
                    watched.insert(path, mtime);
                }
                Ok(FileWatchCommand::Unwatch(path)) => {
                    watched.remove(&path);
                }
                Ok(FileWatchCommand::WatchWorkspace(root)) => {
                    scan_workspace(&root, &mut watched);
                    workspace_root = Some(root);
                    last_workspace_scan = std::time::Instant::now();
                }
                Ok(FileWatchCommand::Shutdown) => return,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        // Periodically rescan workspace for new files.
        if let Some(ref root) = workspace_root {
            if last_workspace_scan.elapsed() >= workspace_scan_interval {
                scan_workspace(root, &mut watched);
                last_workspace_scan = std::time::Instant::now();
            }
        }

        // Check each watched file for mtime changes.
        // Collect changed paths separately to avoid borrow issues.
        let mut changed = Vec::new();
        for (path, last_mtime) in watched.iter_mut() {
            let current = file_mtime(path);
            if current != *last_mtime && current.is_some() {
                *last_mtime = current;
                changed.push(path.clone());
            }
        }
        for path in changed {
            let _ = event_tx.send(path);
        }

        // Sleep, but also listen for commands to stay responsive to shutdown.
        match cmd_rx.recv_timeout(poll_interval) {
            Ok(FileWatchCommand::Watch(path)) => {
                let mtime = file_mtime(&path);
                watched.insert(path, mtime);
            }
            Ok(FileWatchCommand::Unwatch(path)) => {
                watched.remove(&path);
            }
            Ok(FileWatchCommand::WatchWorkspace(root)) => {
                scan_workspace(&root, &mut watched);
                workspace_root = Some(root);
                last_workspace_scan = std::time::Instant::now();
            }
            Ok(FileWatchCommand::Shutdown) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Scan workspace directory for source files, adding new ones to the watch set.
fn scan_workspace(root: &Path, watched: &mut HashMap<PathBuf, Option<SystemTime>>) {
    let walker = WalkDir::new(root);
    for path in walker {
        if watched.contains_key(&path) {
            continue;
        }
        let mtime = file_mtime(&path);
        watched.insert(path, mtime);
    }
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Simple recursive directory walker that yields source files,
/// skipping hidden dirs, .git, target, node_modules, etc.
struct WalkDir {
    stack: Vec<PathBuf>,
}

impl WalkDir {
    fn new(root: &Path) -> Self {
        Self {
            stack: vec![root.to_path_buf()],
        }
    }
}

impl Iterator for WalkDir {
    type Item = PathBuf;

    fn next(&mut self) -> Option<PathBuf> {
        loop {
            let path = self.stack.pop()?;
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with('.')
                    || name == "target"
                    || name == "node_modules"
                    || name == "__pycache__"
                    || name == "vendor"
                    || name == ".git"
                {
                    continue;
                }
                if let Ok(entries) = std::fs::read_dir(&path) {
                    for entry in entries.flatten() {
                        self.stack.push(entry.path());
                    }
                }
                continue;
            }
            if is_source_file(&path) {
                return Some(path);
            }
        }
    }
}

fn is_source_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| SOURCE_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}
