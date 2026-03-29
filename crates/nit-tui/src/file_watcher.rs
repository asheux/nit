//! Lightweight background file watcher that monitors open editor files for
//! external changes (e.g. agent writes) using mtime polling.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

/// Commands sent to the watcher thread.
pub enum FileWatchCommand {
    /// Start watching a file (replaces any previous watch for the same path).
    Watch(PathBuf),
    /// Stop watching a file.
    Unwatch(PathBuf),
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

    /// Convenience: watch a single file (the active editor buffer).
    pub fn watch(&self, path: PathBuf) {
        self.send(FileWatchCommand::Watch(path));
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(FileWatchCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Background loop: polls watched files every ~200ms for mtime changes.
fn watcher_loop(cmd_rx: Receiver<FileWatchCommand>, event_tx: Sender<PathBuf>) {
    let mut watched: HashMap<PathBuf, Option<SystemTime>> = HashMap::new();
    let poll_interval = Duration::from_millis(200);

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
                Ok(FileWatchCommand::Shutdown) => return,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        // Check each watched file for mtime changes.
        for (path, last_mtime) in watched.iter_mut() {
            let current = file_mtime(path);
            if current != *last_mtime && current.is_some() {
                *last_mtime = current;
                let _ = event_tx.send(path.clone());
            }
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
            Ok(FileWatchCommand::Shutdown) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn file_mtime(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}
