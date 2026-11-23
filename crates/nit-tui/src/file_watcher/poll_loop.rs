use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

use super::{parse_gitignore_dirs, walker::SourceTreeWalker, FileWatchCommand};

const POLL_INTERVAL: Duration = Duration::from_millis(200);
const RESCAN_INTERVAL: Duration = Duration::from_secs(2);
const INITIAL_TRACKING_CAPACITY: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordedMtime(Option<SystemTime>);

impl RecordedMtime {
    fn probe(path: &Path) -> Self {
        Self(std::fs::metadata(path).ok().and_then(|m| m.modified().ok()))
    }
}

type MtimeMap = HashMap<PathBuf, RecordedMtime>;

/// Each tick: drain queued commands → optionally re-scan workspace →
/// compare tracked mtimes → emit change events → sleep until next tick.
pub(super) struct PollLoop {
    watched_files: MtimeMap,
    workspace_files: MtimeMap,
    workspace_root: Option<PathBuf>,
    gitignored_dirs: Vec<String>,
    last_scan_timestamp: Instant,
    command_receiver: Receiver<FileWatchCommand>,
    change_emitter: Sender<PathBuf>,
}

impl PollLoop {
    pub(super) fn initialize(
        command_rx: Receiver<FileWatchCommand>,
        event_tx: Sender<PathBuf>,
    ) -> Self {
        Self {
            watched_files: HashMap::new(),
            workspace_files: HashMap::with_capacity(INITIAL_TRACKING_CAPACITY),
            workspace_root: None,
            gitignored_dirs: Vec::new(),
            last_scan_timestamp: Instant::now(),
            command_receiver: command_rx,
            change_emitter: event_tx,
        }
    }

    pub(super) fn run_until_shutdown(&mut self) {
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
                self.watched_files.insert(filepath, baseline);
                false
            }
            FileWatchCommand::Unwatch(filepath) => {
                self.watched_files.remove(&filepath);
                false
            }
            FileWatchCommand::WatchWorkspace(root) => {
                self.gitignored_dirs = parse_gitignore_dirs(&root);
                self.scan_workspace(&root, false);
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
        // picks them up immediately rather than waiting for a future mtime
        // change.
        self.scan_workspace(&workspace, true);
        self.last_scan_timestamp = Instant::now();
    }

    fn scan_workspace(&mut self, root: &Path, notify: bool) {
        let mut previous = std::mem::take(&mut self.workspace_files);
        let mut next = HashMap::with_capacity(previous.len());
        for path in SourceTreeWalker::rooted_at(root, self.gitignored_dirs.clone()) {
            let current = RecordedMtime::probe(&path);
            let old = previous.remove(&path);
            if notify && old.as_ref() != Some(&current) && !self.watched_files.contains_key(&path) {
                let _ = self.change_emitter.send(path.clone());
            }
            next.insert(path, current);
        }
        if notify {
            for path in previous.into_keys() {
                if !self.watched_files.contains_key(&path) && !path.exists() {
                    let _ = self.change_emitter.send(path);
                }
            }
        }
        self.workspace_files = next;
    }

    fn emit_detected_changes(&mut self) {
        for (filepath, recorded) in self.watched_files.iter_mut() {
            let current = RecordedMtime::probe(filepath);
            if current == *recorded {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_workspace_creation_and_removal_without_startup_events() {
        let dir = std::env::temp_dir().join(format!("nit-watch-changes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("original.py");
        let added = dir.join("added.rs");
        std::fs::write(&original, "").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dir, dir.join("recursive")).unwrap();

        let (_, commands) = mpsc::channel();
        let (tx, events) = mpsc::channel();
        let mut watch = PollLoop::initialize(commands, tx);
        watch.execute(FileWatchCommand::WatchWorkspace(dir.clone()));
        assert_eq!(watch.workspace_files.len(), 1);
        assert!(events.try_recv().is_err());
        std::fs::remove_file(&original).unwrap();
        std::fs::write(&added, "").unwrap();
        watch.scan_workspace(&dir, true);
        assert_eq!(events.try_iter().collect::<Vec<_>>(), [added, original]);
        watch.scan_workspace(&dir, true);
        assert!(events.try_recv().is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
