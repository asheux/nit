//! Read-only working-tree review. Only one request can be in flight. Git
//! work and pipe reads stay outside the input/render thread.
use std::{
    ffi::{OsStr, OsString},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use nit_core::state::{ChangeReview, ChangedFile};

const MAX_FILES: usize = 500;
const MAX_PATCH: usize = 32 * 1024;
const MAX_STATUS: usize = 256 * 1024;

#[derive(Default)]
pub(crate) struct ReviewRuntime {
    pending: Option<Receiver<Result<ChangeReview, String>>>,
    requested_path: Option<PathBuf>,
    next_refresh: Option<Instant>,
}

fn selected_path(review: &ChangeReview) -> Option<&Path> {
    review.files.get(review.selected).map(|f| f.path.as_path())
}

impl ReviewRuntime {
    pub(crate) fn tick(&mut self, workspace: &Path, review: &mut ChangeReview) {
        if let Some(rx) = &self.pending {
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => Err("Git review worker stopped".into()),
            };
            self.pending = None;
            if self.requested_path.as_deref() != selected_path(review) {
                review.refresh = true;
            } else {
                let mut snapshot = result.unwrap_or_else(|err| ChangeReview {
                    message: Some(err),
                    ..ChangeReview::default()
                });
                if snapshot.preview_path == review.preview_path {
                    snapshot.scroll = review.scroll.min(snapshot.preview.len().saturating_sub(1));
                }
                snapshot.refresh = review.refresh;
                *review = snapshot;
                let delay = if review.message.is_some() { 12 } else { 3 };
                self.next_refresh = Some(Instant::now() + Duration::from_secs(delay));
            }
        }
        if !review.refresh && self.next_refresh.is_some_and(|due| Instant::now() < due) {
            return;
        }
        review.refresh = false;
        let workspace = workspace.to_path_buf();
        let preferred = selected_path(review).map(Path::to_owned);
        self.requested_path.clone_from(&preferred);
        let (tx, rx) = mpsc::channel();
        if self.next_refresh.is_none() && review.files.is_empty() {
            review.message = Some("Reading Git status…".into());
        }
        match thread::Builder::new()
            .name("nit-git-review".into())
            .spawn(move || {
                let _ = tx.send(read_review(&workspace, preferred.as_deref()));
            }) {
            Ok(_) => self.pending = Some(rx),
            Err(err) => {
                review.message = Some(err.to_string());
                self.next_refresh = Some(Instant::now() + Duration::from_secs(3));
            }
        }
    }
}

struct GitOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

fn git<I, S>(cwd: &Path, args: I, cap: usize) -> Result<GitOutput, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut child = Command::new("git")
        .args([
            "--no-pager",
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
        ])
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Cannot start Git: {err}"))?;
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .expect("piped stdout")
        .take((cap + 1) as u64)
        .read_to_end(&mut bytes);
    let truncated = bytes.len() > cap;
    if truncated || read.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|err| err.to_string())?;
    read.map_err(|err| err.to_string())?;
    if !status.success() && !truncated {
        return Err("Git could not read this working tree".into());
    }
    bytes.truncate(cap);
    Ok(GitOutput { bytes, truncated })
}

fn os_path(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).as_ref())
    }
}

fn read_review(workspace: &Path, preferred: Option<&Path>) -> Result<ChangeReview, String> {
    let location = git(workspace, ["rev-parse", "--show-toplevel"], 8192).map_err(|err| {
        if err.starts_with("Cannot start") {
            err
        } else {
            "No readable Git worktree here".into()
        }
    })?;
    let root = os_path(
        location
            .bytes
            .strip_suffix(b"\n")
            .unwrap_or(&location.bytes),
    );
    let status = git(
        &root,
        [
            "status",
            "--porcelain=v1",
            "-z",
            "--no-renames",
            "--untracked-files=all",
        ],
        MAX_STATUS,
    )?;
    let mut files = Vec::new();
    let mut truncated = status.truncated;
    for record in status.bytes.split_inclusive(|b| *b == 0) {
        let Some(record) = record.strip_suffix(&[0]) else {
            break;
        };
        if record.len() < 4 {
            continue;
        }
        if files.len() == MAX_FILES {
            truncated = true;
            break;
        }
        files.push(ChangedFile {
            path: os_path(&record[3..]),
            index: char::from(record[0]),
            worktree: char::from(record[1]),
        });
    }
    let selected = files
        .iter()
        .position(|file| Some(file.path.as_path()) == preferred)
        .unwrap_or(0);
    let mut review = ChangeReview {
        root,
        files,
        selected,
        truncated,
        ..ChangeReview::default()
    };
    if let Some(file) = review.files.get(selected) {
        review.preview_path = Some(file.path.clone());
        if file.index == '?' {
            review
                .preview
                .push("Untracked file. Press Enter to inspect.".into());
        } else {
            for (status, cached, label) in [
                (file.index, true, "Staged"),
                (file.worktree, false, "Unstaged"),
            ] {
                if status == ' ' {
                    continue;
                }
                let mut args: Vec<OsString> = [
                    "diff",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-renames",
                ]
                .into_iter()
                .map(Into::into)
                .collect();
                if cached {
                    args.push("--cached".into());
                }
                args.push("--".into());
                args.push(file.path.clone().into_os_string());
                review.preview.push(label.into());
                match git(&review.root, args, MAX_PATCH) {
                    Ok(output) => {
                        let patch = String::from_utf8_lossy(&output.bytes);
                        review.preview.extend(
                            patch
                                .lines()
                                .skip_while(|line| {
                                    line.starts_with("diff --git ") || line.starts_with("index ")
                                })
                                .map(str::to_owned),
                        );
                        if output.truncated {
                            review.preview.push("… patch preview truncated".into());
                        }
                    }
                    Err(err) => review.preview.push(err),
                }
            }
        }
    }
    Ok(review)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_index_and_worktree_patches_for_a_literal_filename() {
        let dir = std::env::temp_dir().join(format!("nit-review-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = Path::new("a[1] review.py");
        git(&dir, ["init", "-q"], 1024).unwrap();
        std::fs::write(dir.join(path), "staged\n").unwrap();
        git(&dir, ["add", "--", path.to_str().unwrap()], 1024).unwrap();
        std::fs::write(dir.join(path), "updated\n").unwrap();
        std::fs::write(dir.join("notes.md"), "untracked\n").unwrap();
        let review = read_review(&dir, Some(path)).unwrap();
        assert_eq!(
            (review.files[0].index, review.files[0].worktree),
            ('A', 'M')
        );
        assert_eq!(review.files[1].index, '?');
        let diff = review.preview.join("\n");
        assert!(diff.contains("Staged\n") && diff.contains("+staged"));
        assert!(diff.contains("Unstaged\n") && diff.contains("+updated"));
        let note = read_review(&dir, Some(Path::new("notes.md"))).unwrap();
        assert_eq!(note.selected, 1);
        assert_eq!(note.preview_path.as_deref(), Some(Path::new("notes.md")));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
