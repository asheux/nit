use std::collections::HashMap;
use std::fmt;
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

/// Each tick: drain queued commands → optionally re-scan workspace →
/// compare tracked mtimes → emit change events → sleep until next tick.
pub(super) struct PollLoop {
    tracked_files: MtimeMap,
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
            tracked_files: HashMap::with_capacity(INITIAL_TRACKING_CAPACITY),
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
        // picks them up immediately rather than waiting for a future mtime
        // change.
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
