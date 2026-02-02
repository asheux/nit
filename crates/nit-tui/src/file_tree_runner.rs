use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use nit_core::DirEntryModel;

pub enum FileTreeCommand {
    ListDir {
        dir: PathBuf,
        show_hidden: bool,
        show_ignored: bool,
    },
    Shutdown,
}

pub enum FileTreeEvent {
    DirListed {
        dir: PathBuf,
        entries: Vec<DirEntryModel>,
    },
    Error {
        dir: PathBuf,
        message: String,
    },
}

pub struct FileTreeRunner {
    cmd_tx: Sender<FileTreeCommand>,
    pub events: Receiver<FileTreeEvent>,
    handle: Option<JoinHandle<()>>,
}

impl FileTreeRunner {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("nit-file-tree".into())
            .spawn(move || runner_loop(cmd_rx, event_tx))
            .expect("spawn file tree runner");
        Self {
            cmd_tx,
            events: event_rx,
            handle: Some(handle),
        }
    }

    pub fn send(&self, command: FileTreeCommand) {
        let _ = self.cmd_tx.send(command);
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(FileTreeCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn runner_loop(cmd_rx: Receiver<FileTreeCommand>, event_tx: Sender<FileTreeEvent>) {
    loop {
        match cmd_rx.recv() {
            Ok(FileTreeCommand::ListDir {
                dir,
                show_hidden,
                show_ignored,
            }) => match list_dir(&dir, show_hidden, show_ignored) {
                Ok(entries) => {
                    let _ = event_tx.send(FileTreeEvent::DirListed { dir, entries });
                }
                Err(message) => {
                    let _ = event_tx.send(FileTreeEvent::Error { dir, message });
                }
            },
            Ok(FileTreeCommand::Shutdown) | Err(_) => break,
        }
    }
}

fn list_dir(
    dir: &Path,
    show_hidden: bool,
    show_ignored: bool,
) -> Result<Vec<DirEntryModel>, String> {
    let mut entries = Vec::new();
    let mut candidates_for_git = Vec::new(); // (input bytes, name for filtering)

    let read_dir =
        fs::read_dir(dir).map_err(|e| format!("Failed to read {}: {e}", dir.display()))?;
    for entry in read_dir {
        let entry = entry.map_err(|e| e.to_string())?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        if !show_hidden && name.starts_with('.') {
            continue;
        }

        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        let is_dir = file_type.is_dir();
        let is_symlink = file_type.is_symlink();
        let path = entry.path();

        entries.push(DirEntryModel {
            name: name.clone(),
            path,
            is_dir,
            is_symlink,
        });

        if !show_ignored {
            let mut bytes = name.into_bytes();
            if is_dir {
                bytes.push(b'/');
            }
            candidates_for_git.push(bytes);
        }
    }

    if !show_ignored && !entries.is_empty() {
        let ignored = git_check_ignore(dir, &candidates_for_git).unwrap_or_default();
        if !ignored.is_empty() {
            entries.retain(|e| {
                let mut key = e.name.clone();
                if e.is_dir {
                    key.push('/');
                }
                !ignored.contains(key.as_bytes())
            });
        }
    }

    entries.sort_by_cached_key(|e| (!e.is_dir, e.name.to_ascii_lowercase()));
    Ok(entries)
}

fn git_check_ignore(dir: &Path, paths: &[Vec<u8>]) -> Result<HashSet<Vec<u8>>, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("check-ignore")
        .arg("-z")
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("git check-ignore spawn failed: {e}"))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "git check-ignore stdin unavailable".to_string())?;
        for path in paths {
            stdin
                .write_all(path)
                .and_then(|_| stdin.write_all(b"\0"))
                .map_err(|e| format!("git check-ignore stdin write failed: {e}"))?;
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("git check-ignore wait failed: {e}"))?;

    match output.status.code() {
        // 0: at least one path is ignored; 1: none are ignored; 128: not a git repo / error.
        Some(0) | Some(1) => {}
        _ => return Ok(HashSet::new()),
    }

    let mut ignored = HashSet::new();
    for part in output.stdout.split(|b| *b == 0) {
        if part.is_empty() {
            continue;
        }
        ignored.insert(part.to_vec());
    }
    Ok(ignored)
}
