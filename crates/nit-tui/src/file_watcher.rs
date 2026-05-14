//! Background file-watcher for workspace change detection via mtime
//! polling. Extension/filename gates derive from `nit_core::languages`.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

mod gitignore;
mod poll_loop;
mod walker;

pub use gitignore::parse_gitignore_dirs;

use poll_loop::PollLoop;
use walker::SourceTreeWalker;

pub const IGNORED_DIRS: &[&str] = &["target", "node_modules", "__pycache__", "vendor", ".git"];

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

/// Main-thread handle. Drop without `shutdown()` leaves the thread alive
/// until its channel disconnects.
pub struct FileWatcher {
    command_channel: Sender<FileWatchCommand>,
    pub events: Receiver<PathBuf>,
    watcher_thread: Option<JoinHandle<()>>,
}

impl FileWatcher {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (evt_tx, evt_rx) = mpsc::channel();
        let watcher_thread = thread::Builder::new()
            .name("nit-file-watcher".into())
            .spawn(move || PollLoop::initialize(cmd_rx, evt_tx).run_until_shutdown())
            .expect("failed to spawn file-watcher thread");
        Self {
            command_channel: cmd_tx,
            events: evt_rx,
            watcher_thread: Some(watcher_thread),
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
        self.send(FileWatchCommand::Shutdown);
        if let Some(joinable) = self.watcher_thread.take() {
            let _ = joinable.join();
        }
    }
}

/// True for any path whose extension/filename the central languages table
/// recognises, plus the markup-auxiliary set the editor tracks for
/// buffer-reload hooks even though the genome scan skips them.
pub fn is_trackable_source(filepath: &Path) -> bool {
    if nit_core::languages::detect_by_path(filepath).is_some() {
        return true;
    }
    filepath
        .extension()
        .and_then(|raw| raw.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .is_some_and(|ext| {
            nit_core::languages::MARKUP_AUXILIARY_EXTENSIONS
                .iter()
                .any(|aux| *aux == ext)
        })
}

pub fn is_excluded_directory(dir_component: &str, gitignored: &[String]) -> bool {
    dir_component.starts_with('.')
        || IGNORED_DIRS.contains(&dir_component)
        || gitignored.iter().any(|g| g == dir_component)
}

/// Walk every source file under `root`, skipping gitignored and
/// `IGNORED_DIRS` directories. Used by the workspace-wide genome scan at
/// launch and by tests that need a deterministic fixture walk.
pub fn walk_source_files(root: &Path, gitignored: &[String]) -> Vec<PathBuf> {
    SourceTreeWalker::rooted_at(root, gitignored.to_vec()).collect()
}
