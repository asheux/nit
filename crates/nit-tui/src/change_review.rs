//! Read-only working-tree review. Only one request can be in flight. Git
//! work and pipe reads stay outside the input/render thread.
use std::{
    collections::VecDeque,
    ffi::{OsStr, OsString},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use nit_core::state::{ChangeReview, ChangedFile, ReviewCommit};

const MAX_FILES: usize = 500;
const MAX_PATCH: usize = 32 * 1024;
const MAX_STATUS: usize = 256 * 1024;

const HISTORY_LIMIT: usize = 24;
const PATCH_CACHE_LIMIT: usize = 8;

#[derive(Default)]
struct HistoryCache {
    root: PathBuf,
    head: String,
    commits: Vec<ReviewCommit>,
    patches: VecDeque<(String, Vec<String>)>,
}

#[derive(Clone, PartialEq, Eq)]
enum Target {
    File(PathBuf),
    Commit(String),
}

fn selected_target(review: &ChangeReview) -> Option<Target> {
    if let Some(file) = review.files.get(review.selected) {
        Some(Target::File(file.path.clone()))
    } else {
        review
            .commits
            .get(review.selected)
            .map(|commit| Target::Commit(commit.id.clone()))
    }
}

type WorkerReply = (Result<ChangeReview, String>, HistoryCache);

#[derive(Default)]
pub(crate) struct ReviewRuntime {
    pending: Option<Receiver<WorkerReply>>,
    requested: Option<Target>,
    next_refresh: Option<Instant>,
    cache: HistoryCache,
}

impl ReviewRuntime {
    pub(crate) fn tick(&mut self, workspace: &Path, review: &mut ChangeReview) {
        if let Some(rx) = &self.pending {
            let result = match rx.try_recv() {
                Ok((result, cache)) => {
                    self.cache = cache;
                    result
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => Err("Git review worker stopped".into()),
            };
            self.pending = None;
            if self.requested != selected_target(review) {
                review.refresh = true;
            } else {
                let mut snapshot = result.unwrap_or_else(|err| ChangeReview {
                    message: Some(err),
                    ..ChangeReview::default()
                });
                if snapshot.preview_commit.is_some()
                    && selected_target(&snapshot) == selected_target(review)
                {
                    snapshot.history_patch_open = review.history_patch_open;
                }
                if snapshot.preview_path == review.preview_path
                    && snapshot.preview_commit == review.preview_commit
                {
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
        self.requested = selected_target(review);
        let preferred = self.requested.clone();
        let follow_head = review.files.is_empty() && review.selected == 0;
        let mut cache = std::mem::take(&mut self.cache);
        let (tx, rx) = mpsc::channel();
        if self.next_refresh.is_none() && review.files.is_empty() {
            review.message = Some("Reading Git status…".into());
        }
        match thread::Builder::new()
            .name("nit-git-review".into())
            .spawn(move || {
                let result = read_review(&workspace, preferred.as_ref(), follow_head, &mut cache);
                let _ = tx.send((result, cache));
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
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "")
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

fn read_review(
    workspace: &Path,
    preferred: Option<&Target>,
    follow_head: bool,
    cache: &mut HistoryCache,
) -> Result<ChangeReview, String> {
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
            "--porcelain=v2",
            "--ahead-behind",
            "--branch",
            "-z",
            "--no-renames",
            "--untracked-files=all",
        ],
        MAX_STATUS,
    )?;
    let mut review = ChangeReview {
        root,
        truncated: status.truncated,
        ..Default::default()
    };
    let mut head = String::new();
    for record in status.bytes.split_inclusive(|b| *b == 0) {
        let Some(record) = record.strip_suffix(&[0]) else {
            break;
        };
        if let Some(header) = record.strip_prefix(b"# branch.") {
            let header = String::from_utf8_lossy(header);
            if let Some((key, value)) = header.split_once(' ') {
                match key {
                    "oid" => {
                        review.unborn = value == "(initial)";
                        head = value.to_owned();
                    }
                    "head" => {
                        review.detached = value == "(detached)";
                        review.branch = if review.detached {
                            "HEAD".into()
                        } else {
                            value.into()
                        };
                    }
                    "upstream" => {
                        review.upstream = Some(value.into());
                        review.upstream_gone = true;
                    }
                    "ab" => {
                        if let Some((ahead, behind)) = value.split_once(' ') {
                            review.ahead = ahead.trim_start_matches('+').parse().unwrap_or(0);
                            review.behind = behind.trim_start_matches('-').parse().unwrap_or(0);
                            review.upstream_gone = false;
                        }
                    }
                    _ => {}
                }
            }
            continue;
        }
        let (index, worktree, path) = match record.first() {
            Some(b'?') => (b'?', b'?', &record[2..]),
            Some(b'1' | b'u') => {
                let fields = if record[0] == b'1' { 9 } else { 11 };
                let Some(path) = record.splitn(fields, |&b| b == b' ').nth(fields - 1) else {
                    continue;
                };
                (record[2], record[3], path)
            }
            _ => continue,
        };
        if review.files.len() == MAX_FILES {
            review.truncated = true;
            break;
        }
        review.files.push(ChangedFile {
            path: os_path(path),
            index: char::from(if index == b'.' { b' ' } else { index }),
            worktree: char::from(if worktree == b'.' { b' ' } else { worktree }),
        });
    }
    if let Some(Target::File(path)) = preferred {
        review.selected = review
            .files
            .iter()
            .position(|f| &f.path == path)
            .unwrap_or(0);
    }
    if review.files.is_empty() && !review.unborn && !review.truncated && valid_oid(&head) {
        populate_history(&mut review, &head, preferred, follow_head, cache)?;
    }
    let selected = review.selected;
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
                if status == ' ' || (cached && file.conflicted()) {
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
                review.preview.push(
                    if file.conflicted() {
                        "Merge conflict"
                    } else {
                        label
                    }
                    .into(),
                );
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

fn valid_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn populate_history(
    review: &mut ChangeReview,
    head: &str,
    preferred: Option<&Target>,
    follow_head: bool,
    cache: &mut HistoryCache,
) -> Result<(), String> {
    if cache.root != review.root {
        *cache = HistoryCache {
            root: review.root.clone(),
            ..Default::default()
        };
    }
    if cache.head != head {
        let output = git(
            &review.root,
            [
                "log",
                "--no-show-signature",
                "-z",
                &format!("-{HISTORY_LIMIT}"),
                "--format=%H%x00%<(56,trunc)%an%x00%as%x00%<(160,trunc)%s",
                head,
                "--",
            ],
            MAX_STATUS,
        )?;
        let fields: Vec<_> = output.bytes.split(|&b| b == 0).collect();
        cache.commits = fields
            .chunks_exact(4)
            .filter_map(|commit| {
                let id = std::str::from_utf8(commit[0]).ok()?;
                valid_oid(id).then(|| ReviewCommit {
                    id: id.into(),
                    author: String::from_utf8_lossy(commit[1]).trim_end().to_owned(),
                    date: String::from_utf8_lossy(commit[2]).into_owned(),
                    subject: String::from_utf8_lossy(commit[3]).trim_end().to_owned(),
                })
            })
            .collect();
        cache.head = head.into();
    }
    review.commits.clone_from(&cache.commits);
    if !follow_head {
        if let Some(Target::Commit(id)) = preferred {
            review.selected = review
                .commits
                .iter()
                .position(|commit| &commit.id == id)
                .unwrap_or(0);
        }
    }
    if let Some(commit) = review.commits.get(review.selected) {
        review.preview_commit = Some(commit.id.clone());
        if let Some((_, rows)) = cache.patches.iter().find(|(id, _)| id == &commit.id) {
            review.preview.clone_from(rows);
        } else {
            let result = git(
                &review.root,
                [
                    "show",
                    "--no-show-signature",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-renames",
                    "--format=",
                    "--first-parent",
                    "--root",
                    &commit.id,
                    "--",
                ],
                MAX_PATCH,
            );
            match result {
                Ok(output) => {
                    review.preview = String::from_utf8_lossy(&output.bytes)
                        .lines()
                        .map(str::to_owned)
                        .collect();
                    if output.truncated {
                        review.preview.push("… patch preview truncated".into());
                    }
                    if review.preview.is_empty() {
                        review
                            .preview
                            .push("This commit has no file changes.".into());
                    }
                    if cache.patches.len() == PATCH_CACHE_LIMIT {
                        cache.patches.pop_front();
                    }
                    cache
                        .patches
                        .push_back((commit.id.clone(), review.preview.clone()));
                }
                Err(err) => review.preview.push(err),
            }
        }
    }
    Ok(())
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
        let review = read_review(
            &dir,
            Some(&Target::File(path.into())),
            false,
            &mut HistoryCache::default(),
        )
        .unwrap();
        assert_eq!(
            (review.files[0].index, review.files[0].worktree),
            ('A', 'M')
        );
        assert_eq!(review.files[1].index, '?');
        let diff = review.preview.join("\n");
        assert!(diff.contains("Staged\n") && diff.contains("+staged"));
        assert!(diff.contains("Unstaged\n") && diff.contains("+updated"));
        let note = read_review(
            &dir,
            Some(&Target::File("notes.md".into())),
            false,
            &mut HistoryCache::default(),
        )
        .unwrap();
        assert_eq!(note.selected, 1);
        assert_eq!(note.preview_path.as_deref(), Some(Path::new("notes.md")));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn tracks_clean_history_upstream_and_selection_across_commits() {
        let dir = std::env::temp_dir().join(format!("nit-history-track-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| git(&dir, args.iter().copied(), MAX_STATUS).unwrap();
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.name", "Review Tester"]);
        run(&["config", "user.email", "test@example.invalid"]);
        run(&["config", "commit.gpgsign", "false"]);
        let mut cache = HistoryCache::default();
        let read = |cache: &mut HistoryCache| read_review(&dir, None, true, cache).unwrap();
        let unborn = read(&mut cache);
        assert!(unborn.unborn && unborn.commits.is_empty());
        std::fs::write(dir.join("file"), "initial\n").unwrap();
        run(&["add", "file"]);
        run(&["commit", "-qm", "initial"]);
        let first = read(&mut cache);
        assert_eq!(first.branch, "main");
        assert!(first.upstream.is_none());
        assert_eq!(first.commits[0].author, "Review Tester");
        assert_eq!(first.commits[0].date.len(), 10);
        assert!(first.preview.iter().any(|line| line == "+initial"));
        let preferred = Target::Commit(first.commits[0].id.clone());
        run(&["branch", "tracking"]);
        run(&["branch", "--set-upstream-to=tracking"]);
        let synced = read(&mut cache);
        assert_eq!(synced.upstream.as_deref(), Some("tracking"));
        assert!(!synced.upstream_gone);
        assert_eq!((synced.ahead, synced.behind), (0, 0));
        run(&["commit", "--allow-empty", "-qm", "next"]);
        let ahead = read(&mut cache);
        assert_eq!(ahead.commits[0].subject, "next");
        assert_eq!((ahead.ahead, ahead.behind), (1, 0));
        let older = read_review(&dir, Some(&preferred), false, &mut cache).unwrap();
        assert_eq!(older.selected, 1);
        assert_eq!(older.preview_commit, first.preview_commit);
        assert_eq!(older.preview, first.preview);
        run(&["checkout", "-q", "tracking"]);
        run(&["commit", "--allow-empty", "-qm", "other"]);
        run(&["checkout", "-q", "main"]);
        let diverged = read(&mut cache);
        assert_eq!((diverged.ahead, diverged.behind), (1, 1));
        run(&["branch", "-D", "tracking"]);
        // Reattach the local upstream configuration which branch deletion removes.
        run(&["config", "branch.main.remote", "."]);
        run(&["config", "branch.main.merge", "refs/heads/tracking"]);
        let gone = read(&mut cache);
        assert_eq!(gone.upstream.as_deref(), Some("tracking"));
        assert!(gone.upstream_gone);
        run(&["checkout", "-q", "--detach"]);
        let detached = read(&mut cache);
        assert!(detached.detached && detached.upstream.is_none());
        assert_eq!(detached.branch, "HEAD");
        assert_eq!(detached.commits.len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn bounds_clean_history_and_cached_patches() {
        let dir = std::env::temp_dir().join(format!("nit-history-bounds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| git(&dir, args.iter().copied(), MAX_STATUS).unwrap();
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.invalid"]);
        run(&["config", "commit.gpgsign", "false"]);
        for i in 0..HISTORY_LIMIT {
            run(&["commit", "--allow-empty", "-qm", &i.to_string()]);
        }
        std::fs::write(dir.join("long.txt"), "patch row\n".repeat(MAX_PATCH / 4)).unwrap();
        run(&["add", "long.txt"]);
        run(&["commit", "-qm", "long patch"]);
        let mut cache = HistoryCache::default();
        let initial = read_review(&dir, None, true, &mut cache).unwrap();
        assert_eq!(initial.commits.len(), HISTORY_LIMIT);
        assert_eq!(initial.commits.last().unwrap().subject, "1");
        assert_eq!(initial.preview.last().unwrap(), "… patch preview truncated");
        for commit in &initial.commits[1..=PATCH_CACHE_LIMIT] {
            let target = Target::Commit(commit.id.clone());
            let review = read_review(&dir, Some(&target), false, &mut cache).unwrap();
            assert_eq!(review.preview, ["This commit has no file changes."]);
        }
        assert_eq!(cache.patches.len(), PATCH_CACHE_LIMIT);
        assert!(!cache
            .patches
            .iter()
            .any(|(id, _)| id == &initial.commits[0].id));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
