//! Background file-watcher for workspace change detection via mtime polling.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

const POLL_INTERVAL: Duration = Duration::from_millis(200);
const RESCAN_INTERVAL: Duration = Duration::from_secs(2);
const INITIAL_TRACKING_CAPACITY: usize = 256;

const IGNORED_DIRS: &[&str] = &["target", "node_modules", "__pycache__", "vendor", ".git"];

const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "c", "cpp", "h", "hpp", "go", "swift", "java", "kt", "scala", "cs", "py", "rb", "sh",
    "bash", "zsh", "js", "jsx", "ts", "tsx", "html", "css", "toml", "yaml", "yml", "json", "sql",
    "md", "txt",
];

#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordedMtime(Option<SystemTime>);

impl RecordedMtime {
    fn probe(path: &Path) -> Self {
        Self(std::fs::metadata(path).ok().and_then(|m| m.modified().ok()))
    }

    const fn is_known(&self) -> bool {
        self.0.is_some()
    }

    fn has_changed_from(&self, baseline: &Self) -> bool {
        self.is_known() && *self != *baseline
    }
}

impl fmt::Display for RecordedMtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(_) => write!(f, "known"),
            None => write!(f, "unknown"),
        }
    }
}

type MtimeMap = HashMap<PathBuf, RecordedMtime>;

pub enum FileWatchCommand {
    Watch(PathBuf),
    Unwatch(PathBuf),
    WatchWorkspace(PathBuf),
    Shutdown,
}

impl fmt::Display for FileWatchCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Watch(path) => write!(f, "Watch({})", path.display()),
            Self::Unwatch(path) => write!(f, "Unwatch({})", path.display()),
            Self::WatchWorkspace(path) => write!(f, "WatchWorkspace({})", path.display()),
            Self::Shutdown => write!(f, "Shutdown"),
        }
    }
}

/// Main-thread handle for the background file-watcher. Drop without
/// `shutdown()` leaves the thread running until its channel disconnects.
pub struct FileWatcher {
    command_channel: Sender<FileWatchCommand>,
    pub events: Receiver<PathBuf>,
    watcher_thread: Option<JoinHandle<()>>,
}

impl FileWatcher {
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

    pub fn send(&self, command: FileWatchCommand) {
        let _ = self.command_channel.send(command);
    }

    pub fn watch(&self, path: PathBuf) {
        self.send(FileWatchCommand::Watch(path));
    }

    pub fn watch_workspace(&self, root: PathBuf) {
        self.send(FileWatchCommand::WatchWorkspace(root));
    }

    pub fn shutdown(&mut self) {
        let _ = self.command_channel.send(FileWatchCommand::Shutdown);
        if let Some(joinable) = self.watcher_thread.take() {
            let _ = joinable.join();
        }
    }
}

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

    /// Returns `true` when the loop should exit.
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
                self.emit_workspace_discoveries(&root);
                self.workspace_root = Some(root);
                self.last_scan_timestamp = Instant::now();
                false
            }
            FileWatchCommand::Shutdown => true,
        }
    }

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

    fn rescan_workspace_if_due(&mut self) {
        let Some(workspace) = self.workspace_root.clone() else {
            return;
        };
        if self.last_scan_timestamp.elapsed() < RESCAN_INTERVAL {
            return;
        }
        // Emit change events for newly discovered files so the genome system
        // picks them up immediately rather than waiting for a future mtime change.
        self.emit_workspace_discoveries(&workspace);
        self.last_scan_timestamp = Instant::now();
    }

    fn emit_workspace_discoveries(&mut self, root: &Path) {
        let new_files = discover_source_files(root, &mut self.tracked_files, &self.gitignored_dirs);
        for path in new_files {
            let _ = self.change_emitter.send(path);
        }
    }

    fn emit_detected_changes(&mut self) {
        for (filepath, recorded) in self.tracked_files.iter_mut() {
            let current = RecordedMtime::probe(filepath);
            if !current.has_changed_from(recorded) {
                continue;
            }
            *recorded = current;
            let _ = self.change_emitter.send(filepath.clone());
        }
    }

    fn sleep_or_handle_command(&mut self) -> bool {
        match self.command_receiver.recv_timeout(POLL_INTERVAL) {
            Ok(incoming) => self.execute(incoming),
            Err(mpsc::RecvTimeoutError::Timeout) => false,
            Err(mpsc::RecvTimeoutError::Disconnected) => true,
        }
    }
}

/// Walk `root` recursively, inserting newly discovered source files into
/// `destination`. Existing entries keep their baseline mtime. Returns the
/// list of paths added on this call.
fn discover_source_files(
    root: &Path,
    destination: &mut MtimeMap,
    gitignored: &[String],
) -> Vec<PathBuf> {
    use std::collections::hash_map::Entry;
    let mut new_paths = Vec::new();
    SourceTreeWalker::rooted_at(root, gitignored.to_vec()).for_each(|discovered_path| {
        if let Entry::Vacant(slot) = destination.entry(discovered_path.clone()) {
            slot.insert(RecordedMtime::probe(&discovered_path));
            new_paths.push(discovered_path);
        }
    });
    new_paths
}

fn is_trackable_source(filepath: &Path) -> bool {
    filepath
        .extension()
        .and_then(|raw| raw.to_str())
        .is_some_and(|ext| SOURCE_EXTENSIONS.contains(&ext))
}

fn is_excluded_directory(dir_component: &str, gitignored: &[String]) -> bool {
    dir_component.starts_with('.')
        || IGNORED_DIRS.contains(&dir_component)
        || gitignored.iter().any(|g| g == dir_component)
}

/// Parse `.gitignore` at `workspace_root` and return simple directory names.
/// Handles only bare-name patterns; ignores globs, negations, and nested
/// `.gitignore` files. Public so startup can pre-populate `AppState.gitignored_dirs`.
pub fn parse_gitignore_dirs(workspace_root: &Path) -> Vec<String> {
    let gitignore_path = workspace_root.join(".gitignore");
    let Ok(content) = std::fs::read_to_string(&gitignore_path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(parse_gitignore_dir_line)
        .collect()
}

fn parse_gitignore_dir_line(raw: &str) -> Option<String> {
    let line = raw.trim();
    if line.is_empty()
        || line.starts_with('#')
        || line.starts_with('!')
        || line.contains('*')
        || line.contains('?')
    {
        return None;
    }
    let trimmed = line.strip_prefix('/').unwrap_or(line);
    let trimmed = trimmed.strip_suffix('/').unwrap_or(trimmed);
    if trimmed.is_empty() || trimmed.contains('/') {
        return None;
    }
    Some(trimmed.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Descendable,
    Trackable,
    Irrelevant,
}

impl EntryKind {
    fn classify(entry_path: &Path) -> Self {
        if entry_path.is_dir() {
            Self::Descendable
        } else if is_trackable_source(entry_path) {
            Self::Trackable
        } else {
            Self::Irrelevant
        }
    }
}

impl fmt::Display for EntryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Descendable => write!(f, "descendable"),
            Self::Trackable => write!(f, "trackable"),
            Self::Irrelevant => write!(f, "irrelevant"),
        }
    }
}

struct SourceTreeWalker {
    pending_paths: Vec<PathBuf>,
    gitignored: Vec<String>,
}

impl SourceTreeWalker {
    fn rooted_at(start: &Path, gitignored: Vec<String>) -> Self {
        Self {
            pending_paths: vec![start.to_path_buf()],
            gitignored,
        }
    }

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
