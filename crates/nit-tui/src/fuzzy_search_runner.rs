use std::collections::BinaryHeap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crossbeam_channel::{unbounded, Receiver, Sender};
use nit_core::{SearchResultFile, SearchResultMatch};

const INDEX_BATCH_SIZE: usize = 400;
const MAX_FILE_RESULTS: usize = 2000;
const MATCH_BATCH_SIZE: usize = 48;
const MAX_MATCH_RESULTS: usize = 2000;
const MAX_SEARCH_FILE_BYTES: u64 = 5 * 1024 * 1024;
const BINARY_SNIFF_BYTES: usize = 8 * 1024;
const SNIPPET_MAX_CHARS: usize = 180;

#[derive(Clone, Debug)]
pub struct IndexedFile {
    pub rel_path: String,
    pub rel_lower: String,
}

pub enum IndexCommand {
    BuildIndex {
        generation: u64,
        root: PathBuf,
        show_hidden: bool,
        show_ignored: bool,
    },
    Shutdown,
}

pub enum IndexEvent {
    Started {
        generation: u64,
    },
    Batch {
        generation: u64,
        files: Vec<IndexedFile>,
        total_indexed: usize,
    },
    Done {
        generation: u64,
        total_files: usize,
        duration_ms: u128,
    },
    Error {
        generation: u64,
        message: String,
    },
}

pub struct FileIndexRunner {
    cmd_tx: Sender<IndexCommand>,
    pub events: Receiver<IndexEvent>,
    handle: Option<JoinHandle<()>>,
    active_generation: Arc<AtomicU64>,
}

impl FileIndexRunner {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let active_generation = Arc::new(AtomicU64::new(0));
        let gen = Arc::clone(&active_generation);
        let handle = thread::Builder::new()
            .name("nit-search-index".into())
            .spawn(move || index_coordinator_loop(cmd_rx, event_tx, gen))
            .expect("spawn index coordinator");
        Self {
            cmd_tx,
            events: event_rx,
            handle: Some(handle),
            active_generation,
        }
    }

    pub fn build(&self, generation: u64, root: PathBuf, show_hidden: bool, show_ignored: bool) {
        self.active_generation.store(generation, Ordering::Relaxed);
        let _ = self.cmd_tx.send(IndexCommand::BuildIndex {
            generation,
            root,
            show_hidden,
            show_ignored,
        });
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(IndexCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn index_coordinator_loop(
    cmd_rx: Receiver<IndexCommand>,
    event_tx: Sender<IndexEvent>,
    active_generation: Arc<AtomicU64>,
) {
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            IndexCommand::BuildIndex {
                generation,
                root,
                show_hidden,
                show_ignored,
            } => {
                active_generation.store(generation, Ordering::Relaxed);
                let tx = event_tx.clone();
                let gen = Arc::clone(&active_generation);
                workers.push(
                    thread::Builder::new()
                        .name(format!("nit-search-index-worker-{generation}"))
                        .spawn(move || {
                            run_index_worker(generation, root, show_hidden, show_ignored, gen, tx)
                        })
                        .expect("spawn index worker"),
                );
            }
            IndexCommand::Shutdown => break,
        }
    }
    active_generation.store(u64::MAX, Ordering::Relaxed);
    for handle in workers {
        let _ = handle.join();
    }
}

fn run_index_worker(
    generation: u64,
    root: PathBuf,
    show_hidden: bool,
    show_ignored: bool,
    active_generation: Arc<AtomicU64>,
    event_tx: Sender<IndexEvent>,
) {
    let start = Instant::now();
    let _ = event_tx.send(IndexEvent::Started { generation });

    let rel_paths = match list_index_paths(&root, show_hidden, show_ignored) {
        Ok(v) => v,
        Err(err) => {
            let _ = event_tx.send(IndexEvent::Error {
                generation,
                message: err,
            });
            return;
        }
    };

    let mut batch: Vec<IndexedFile> = Vec::with_capacity(INDEX_BATCH_SIZE);
    let mut total = 0usize;
    for rel_path in rel_paths {
        if active_generation.load(Ordering::Relaxed) != generation {
            break;
        }
        batch.push(IndexedFile {
            rel_lower: rel_path.to_ascii_lowercase(),
            rel_path,
        });
        total += 1;
        if batch.len() >= INDEX_BATCH_SIZE {
            let files = std::mem::take(&mut batch);
            let _ = event_tx.send(IndexEvent::Batch {
                generation,
                files,
                total_indexed: total,
            });
            batch = Vec::with_capacity(INDEX_BATCH_SIZE);
        }
    }

    if !batch.is_empty() {
        let _ = event_tx.send(IndexEvent::Batch {
            generation,
            files: batch,
            total_indexed: total,
        });
    }

    let _ = event_tx.send(IndexEvent::Done {
        generation,
        total_files: total,
        duration_ms: start.elapsed().as_millis(),
    });
}

fn list_index_paths(
    root: &Path,
    show_hidden: bool,
    show_ignored: bool,
) -> Result<Vec<String>, String> {
    if let Some(mut paths) = git_ls_files(root, show_ignored)? {
        if !show_hidden {
            paths.retain(|p| !is_hidden_rel_path(p));
        }
        return Ok(paths);
    }
    fs_walk_files(root, show_hidden)
}

fn git_ls_files(root: &Path, show_ignored: bool) -> Result<Option<Vec<String>>, String> {
    let mut out = Vec::new();
    let Some(mut main) = run_git_ls_files(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?
    else {
        return Ok(None);
    };
    out.append(&mut main);
    if show_ignored {
        let Some(mut ignored) = run_git_ls_files(
            root,
            &[
                "ls-files",
                "-z",
                "--others",
                "--ignored",
                "--exclude-standard",
            ],
        )?
        else {
            return Ok(None);
        };
        out.append(&mut ignored);
    }
    out.retain(|p| !p.is_empty());
    out.sort();
    out.dedup();
    Ok(Some(out))
}

fn run_git_ls_files(root: &Path, args: &[&str]) -> Result<Option<Vec<String>>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("git spawn failed: {e}"))?;

    match output.status.code() {
        Some(0) => {}
        Some(128) => return Ok(None),
        _ => return Ok(None),
    }

    let mut files = Vec::new();
    for part in output.stdout.split(|b| *b == 0) {
        if part.is_empty() {
            continue;
        }
        files.push(String::from_utf8_lossy(part).replace('\\', "/"));
    }
    Ok(Some(files))
}

fn fs_walk_files(root: &Path, show_hidden: bool) -> Result<Vec<String>, String> {
    let mut stack = vec![root.to_path_buf()];
    let mut out = Vec::new();
    while let Some(dir) = stack.pop() {
        let read_dir =
            fs::read_dir(&dir).map_err(|e| format!("Failed to read {}: {e}", dir.display()))?;
        for entry in read_dir {
            let entry = entry.map_err(|e| e.to_string())?;
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            if name == ".git" {
                continue;
            }
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let ft = entry.file_type().map_err(|e| e.to_string())?;
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                let rel = match path.strip_prefix(root) {
                    Ok(rel) => rel,
                    Err(_) => path.as_path(),
                };
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.retain(|p| !p.is_empty());
    out.sort();
    Ok(out)
}

fn is_hidden_rel_path(rel: &str) -> bool {
    for part in rel.split('/') {
        if part == ".git" {
            return true;
        }
        if part.starts_with('.') {
            return true;
        }
    }
    false
}

pub enum FuzzyCommand {
    ResetIndex {
        generation: u64,
        root: PathBuf,
    },
    IndexBatch {
        generation: u64,
        files: Vec<IndexedFile>,
    },
    IndexDone {
        generation: u64,
    },
    Query {
        generation: u64,
        query: String,
    },
    Shutdown,
}

pub enum FuzzyEvent {
    ResultsReplace {
        generation: u64,
        results: Vec<SearchResultFile>,
        total_indexed: usize,
        total_matches: usize,
        duration_ms: u128,
    },
    ResultsAppend {
        generation: u64,
        results: Vec<SearchResultFile>,
        total_indexed: usize,
    },
}

pub struct FuzzyMatcherRunner {
    cmd_tx: Sender<FuzzyCommand>,
    pub events: Receiver<FuzzyEvent>,
    handle: Option<JoinHandle<()>>,
}

impl FuzzyMatcherRunner {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let handle = thread::Builder::new()
            .name("nit-search-fuzzy".into())
            .spawn(move || fuzzy_loop(cmd_rx, event_tx))
            .expect("spawn fuzzy matcher");
        Self {
            cmd_tx,
            events: event_rx,
            handle: Some(handle),
        }
    }

    pub fn send(&self, cmd: FuzzyCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(FuzzyCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Clone, Debug)]
struct Candidate {
    score: i64,
    idx: usize,
    matched_indices: Vec<usize>,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score && self.idx == other.idx
    }
}

impl Eq for Candidate {}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.score
            .cmp(&other.score)
            .then_with(|| self.idx.cmp(&other.idx))
    }
}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn fuzzy_loop(cmd_rx: Receiver<FuzzyCommand>, event_tx: Sender<FuzzyEvent>) {
    let mut root = PathBuf::new();
    let mut index_generation = 0u64;
    let mut index: Vec<IndexedFile> = Vec::new();

    let mut query_generation = 0u64;
    let mut query = String::new();
    let mut streamed = 0usize;

    while let Ok(cmd) = cmd_rx.recv() {
        let mut recompute = false;
        let mut index_changed = false;
        match cmd {
            FuzzyCommand::ResetIndex {
                generation,
                root: r,
            } => {
                index_generation = generation;
                root = r;
                index.clear();
                query.clear();
                query_generation = generation;
                streamed = 0;
                recompute = true;
            }
            FuzzyCommand::IndexBatch { generation, files } => {
                if generation == index_generation {
                    index.extend(files);
                    index_changed = true;
                }
            }
            FuzzyCommand::IndexDone { .. } => {}
            FuzzyCommand::Query {
                generation,
                query: q,
            } => {
                query_generation = generation;
                query = q;
                recompute = true;
            }
            FuzzyCommand::Shutdown => break,
        }

        // Coalesce bursts.
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                FuzzyCommand::ResetIndex {
                    generation,
                    root: r,
                } => {
                    index_generation = generation;
                    root = r;
                    index.clear();
                    query.clear();
                    query_generation = generation;
                    streamed = 0;
                    recompute = true;
                }
                FuzzyCommand::IndexBatch { generation, files } => {
                    if generation == index_generation {
                        index.extend(files);
                        index_changed = true;
                    }
                }
                FuzzyCommand::IndexDone { .. } => {}
                FuzzyCommand::Query {
                    generation,
                    query: q,
                } => {
                    query_generation = generation;
                    query = q;
                    recompute = true;
                }
                FuzzyCommand::Shutdown => return,
            }
        }

        if query.is_empty() && index_changed && streamed < MAX_FILE_RESULTS && query_generation != 0
        {
            let end = index.len().min(MAX_FILE_RESULTS);
            if end > streamed {
                let mut results = Vec::with_capacity(end - streamed);
                for item in &index[streamed..end] {
                    results.push(SearchResultFile {
                        rel_path: item.rel_path.clone(),
                        abs_path: root.join(&item.rel_path),
                        score: 0,
                        matched_indices: Vec::new(),
                    });
                }
                streamed = end;
                let _ = event_tx.send(FuzzyEvent::ResultsAppend {
                    generation: query_generation,
                    results,
                    total_indexed: index.len(),
                });
            }
        }

        if !recompute {
            continue;
        }

        let start = Instant::now();
        if query.is_empty() {
            let end = index.len().min(MAX_FILE_RESULTS);
            let mut results = Vec::with_capacity(end);
            for item in &index[0..end] {
                results.push(SearchResultFile {
                    rel_path: item.rel_path.clone(),
                    abs_path: root.join(&item.rel_path),
                    score: 0,
                    matched_indices: Vec::new(),
                });
            }
            streamed = end;
            let _ = event_tx.send(FuzzyEvent::ResultsReplace {
                generation: query_generation,
                results,
                total_indexed: index.len(),
                total_matches: index.len(),
                duration_ms: start.elapsed().as_millis(),
            });
            continue;
        }

        let query_lc = query.to_ascii_lowercase();
        let mut total_matches = 0usize;
        let mut heap: BinaryHeap<std::cmp::Reverse<Candidate>> = BinaryHeap::new();
        for (idx, item) in index.iter().enumerate() {
            if let Some((score, indices)) =
                fuzzy_score_bytes(item.rel_lower.as_bytes(), query_lc.as_bytes())
            {
                total_matches += 1;
                let cand = Candidate {
                    score,
                    idx,
                    matched_indices: indices,
                };
                if heap.len() < MAX_FILE_RESULTS {
                    heap.push(std::cmp::Reverse(cand));
                    continue;
                }
                let Some(worst) = heap.peek().map(|r| &r.0) else {
                    continue;
                };
                if cand.score > worst.score {
                    let _ = heap.pop();
                    heap.push(std::cmp::Reverse(cand));
                }
            }
        }
        let mut candidates: Vec<Candidate> = heap.into_iter().map(|r| r.0).collect();
        candidates.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| index[a.idx].rel_lower.cmp(&index[b.idx].rel_lower))
        });
        let mut results = Vec::with_capacity(candidates.len());
        for cand in candidates {
            let item = &index[cand.idx];
            results.push(SearchResultFile {
                rel_path: item.rel_path.clone(),
                abs_path: root.join(&item.rel_path),
                score: cand.score,
                matched_indices: cand.matched_indices,
            });
        }
        let _ = event_tx.send(FuzzyEvent::ResultsReplace {
            generation: query_generation,
            results,
            total_indexed: index.len(),
            total_matches,
            duration_ms: start.elapsed().as_millis(),
        });
    }
}

fn fuzzy_score_bytes(hay: &[u8], needle: &[u8]) -> Option<(i64, Vec<usize>)> {
    if needle.is_empty() {
        return Some((0, Vec::new()));
    }
    let mut indices: Vec<usize> = Vec::with_capacity(needle.len());
    let mut score: i64 = 0;
    let mut h = 0usize;
    let mut last = None;
    for &n in needle {
        while h < hay.len() && hay[h] != n {
            h += 1;
        }
        if h >= hay.len() {
            return None;
        }
        indices.push(h);
        if let Some(prev) = last {
            if h == prev + 1 {
                score += 15;
            } else {
                score += 5;
                score -= (h.saturating_sub(prev) as i64).saturating_sub(1);
            }
        } else {
            score += 10;
            score -= h as i64;
        }
        last = Some(h);
        h += 1;
    }
    score += (needle.len() as i64) * 20;
    score -= (hay.len() as i64) / 10;
    Some((score, indices))
}

pub enum ContentCommand {
    Search {
        generation: u64,
        root: PathBuf,
        query: String,
        show_hidden: bool,
        show_ignored: bool,
    },
    Shutdown,
}

pub enum ContentEvent {
    Started {
        generation: u64,
    },
    MatchBatch {
        generation: u64,
        results: Vec<SearchResultMatch>,
    },
    Done {
        generation: u64,
        total_matches: usize,
        duration_ms: u128,
    },
    Error {
        generation: u64,
        message: String,
    },
}

pub struct ContentSearchRunner {
    cmd_tx: Sender<ContentCommand>,
    pub events: Receiver<ContentEvent>,
    handle: Option<JoinHandle<()>>,
    active_generation: Arc<AtomicU64>,
}

impl ContentSearchRunner {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let active_generation = Arc::new(AtomicU64::new(0));
        let gen = Arc::clone(&active_generation);
        let handle = thread::Builder::new()
            .name("nit-search-content".into())
            .spawn(move || content_coordinator_loop(cmd_rx, event_tx, gen))
            .expect("spawn content coordinator");
        Self {
            cmd_tx,
            events: event_rx,
            handle: Some(handle),
            active_generation,
        }
    }

    pub fn search(
        &self,
        generation: u64,
        root: PathBuf,
        query: String,
        show_hidden: bool,
        show_ignored: bool,
    ) {
        self.active_generation.store(generation, Ordering::Relaxed);
        let _ = self.cmd_tx.send(ContentCommand::Search {
            generation,
            root,
            query,
            show_hidden,
            show_ignored,
        });
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(ContentCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn content_coordinator_loop(
    cmd_rx: Receiver<ContentCommand>,
    event_tx: Sender<ContentEvent>,
    active_generation: Arc<AtomicU64>,
) {
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    while let Ok(first) = cmd_rx.recv() {
        let mut latest = match first {
            ContentCommand::Search { .. } => Some(first),
            ContentCommand::Shutdown => break,
        };
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                ContentCommand::Search { .. } => latest = Some(cmd),
                ContentCommand::Shutdown => {
                    latest = None;
                    break;
                }
            }
        }
        let Some(ContentCommand::Search {
            generation,
            root,
            query,
            show_hidden,
            show_ignored,
        }) = latest
        else {
            break;
        };

        active_generation.store(generation, Ordering::Relaxed);
        let tx = event_tx.clone();
        let gen = Arc::clone(&active_generation);
        workers.push(
            thread::Builder::new()
                .name(format!("nit-search-content-worker-{generation}"))
                .spawn(move || {
                    run_content_worker(generation, root, query, show_hidden, show_ignored, gen, tx)
                })
                .expect("spawn content worker"),
        );
    }
    active_generation.store(u64::MAX, Ordering::Relaxed);
    for handle in workers {
        let _ = handle.join();
    }
}

fn run_content_worker(
    generation: u64,
    root: PathBuf,
    query: String,
    show_hidden: bool,
    show_ignored: bool,
    active_generation: Arc<AtomicU64>,
    event_tx: Sender<ContentEvent>,
) {
    let start = Instant::now();
    let _ = event_tx.send(ContentEvent::Started { generation });

    let needle = query.trim().to_string();
    if needle.is_empty() {
        let _ = event_tx.send(ContentEvent::Done {
            generation,
            total_matches: 0,
            duration_ms: start.elapsed().as_millis(),
        });
        return;
    }

    let rel_paths = match list_index_paths(&root, show_hidden, show_ignored) {
        Ok(v) => v,
        Err(err) => {
            let _ = event_tx.send(ContentEvent::Error {
                generation,
                message: err,
            });
            return;
        }
    };

    let mut batch: Vec<SearchResultMatch> = Vec::with_capacity(MATCH_BATCH_SIZE);
    let mut total_matches = 0usize;
    for rel_path in rel_paths {
        if active_generation.load(Ordering::Relaxed) != generation {
            break;
        }
        let path = root.join(&rel_path);
        if let Ok(meta) = fs::metadata(&path) {
            if meta.len() > MAX_SEARCH_FILE_BYTES {
                continue;
            }
        }
        if is_probably_binary(&path) {
            continue;
        }
        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let mut reader = std::io::BufReader::new(file);
        let mut line = String::new();
        let mut line_no = 0usize;
        loop {
            if active_generation.load(Ordering::Relaxed) != generation {
                break;
            }
            line.clear();
            let n = match std::io::BufRead::read_line(&mut reader, &mut line) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            if n == 0 {
                break;
            }
            line_no += 1;
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            let Some(byte_idx) = line.find(&needle) else {
                continue;
            };
            let start_char = line[..byte_idx].chars().count();
            let match_len = needle.chars().count().max(1);
            let (snippet, match_start) = build_snippet(&line, start_char, match_len);
            batch.push(SearchResultMatch {
                rel_path: rel_path.clone(),
                abs_path: path.clone(),
                line: line_no,
                col: start_char + 1,
                snippet,
                match_start,
                match_len,
            });
            total_matches += 1;
            if total_matches >= MAX_MATCH_RESULTS {
                break;
            }
            if batch.len() >= MATCH_BATCH_SIZE {
                let out = std::mem::take(&mut batch);
                let _ = event_tx.send(ContentEvent::MatchBatch {
                    generation,
                    results: out,
                });
                batch = Vec::with_capacity(MATCH_BATCH_SIZE);
            }
        }
        if total_matches >= MAX_MATCH_RESULTS {
            break;
        }
    }

    if !batch.is_empty() {
        let _ = event_tx.send(ContentEvent::MatchBatch {
            generation,
            results: batch,
        });
    }
    let _ = event_tx.send(ContentEvent::Done {
        generation,
        total_matches,
        duration_ms: start.elapsed().as_millis(),
    });
}

fn is_probably_binary(path: &Path) -> bool {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut reader = std::io::BufReader::new(file);
    let mut buf = vec![0u8; BINARY_SNIFF_BYTES];
    let n = match std::io::Read::read(&mut reader, &mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    buf[..n].contains(&0)
}

fn build_snippet(line: &str, match_start: usize, match_len: usize) -> (String, usize) {
    let total_chars = line.chars().count();
    if total_chars <= SNIPPET_MAX_CHARS {
        return (line.to_string(), match_start);
    }
    let before = SNIPPET_MAX_CHARS / 3;
    let after = SNIPPET_MAX_CHARS
        .saturating_sub(before)
        .saturating_sub(match_len);
    let start_char = match_start.saturating_sub(before);
    let end_char = (match_start + match_len + after).min(total_chars);
    let mut snippet = slice_by_char(line, start_char, end_char);
    let mut adj_start = match_start.saturating_sub(start_char);
    if start_char > 0 {
        snippet.insert(0, '…');
        adj_start += 1;
    }
    if end_char < total_chars {
        snippet.push('…');
    }
    (snippet, adj_start)
}

fn slice_by_char(input: &str, start: usize, end: usize) -> String {
    if start >= end {
        return String::new();
    }
    let mut start_byte = None;
    let mut end_byte = None;
    for (count, (idx, _)) in input.char_indices().enumerate() {
        if count == start {
            start_byte = Some(idx);
        }
        if count == end {
            end_byte = Some(idx);
            break;
        }
    }
    let start_byte = start_byte.unwrap_or(input.len());
    let end_byte = end_byte.unwrap_or(input.len());
    input[start_byte..end_byte].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::AtomicU64;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let dir = std::env::temp_dir()
                .join(format!("nit-test-{prefix}-{}-{nanos}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self { path: dir }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn git_available() -> bool {
        Command::new("git").arg("--version").output().is_ok()
    }

    fn git(cmd: &[&str], dir: &Path) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(cmd)
            .status()
            .expect("git command");
        assert!(status.success(), "git {:?} failed", cmd);
    }

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(path, content).expect("write file");
    }

    #[test]
    fn fuzzy_score_bytes_returns_matched_indices() {
        let hay = b"src/main.rs";
        let needle = b"srs";
        let (score, indices) = fuzzy_score_bytes(hay, needle).expect("match should exist");
        assert!(score > 0);
        assert_eq!(indices, vec![0, 1, 10]);
        assert!(fuzzy_score_bytes(hay, b"zz").is_none());
    }

    #[test]
    fn list_index_paths_respects_gitignore_and_hidden() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new("index");
        git(&["init", "-q"], &tmp.path);

        write(
            &tmp.path.join(".gitignore"),
            "ignored.txt\ntarget/\n# keep\n",
        );
        write(&tmp.path.join("src/main.rs"), "fn main() {}\n");
        write(&tmp.path.join(".env"), "SECRET=1\n");
        write(&tmp.path.join("ignored.txt"), "nope\n");
        write(&tmp.path.join("target/build.log"), "ignored\n");

        let paths = list_index_paths(&tmp.path, false, false).expect("list_index_paths");
        assert!(paths.contains(&"src/main.rs".to_string()));
        assert!(!paths.iter().any(|p| p == ".env"));
        assert!(!paths.iter().any(|p| p == "ignored.txt"));
        assert!(!paths.iter().any(|p| p == "target/build.log"));

        let paths = list_index_paths(&tmp.path, true, false).expect("list_index_paths");
        assert!(paths.contains(&"src/main.rs".to_string()));
        assert!(paths.contains(&".env".to_string()));
        assert!(!paths.iter().any(|p| p == "ignored.txt"));

        let paths = list_index_paths(&tmp.path, true, true).expect("list_index_paths");
        assert!(paths.contains(&"src/main.rs".to_string()));
        assert!(paths.contains(&".env".to_string()));
        assert!(paths.contains(&"ignored.txt".to_string()));
        assert!(paths.contains(&"target/build.log".to_string()));
    }

    #[test]
    fn content_worker_finds_first_match_and_reports_position() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new("content");
        git(&["init", "-q"], &tmp.path);
        write(
            &tmp.path.join("src/main.rs"),
            "fn main() {\n    println!(\"hi\");\n}\n",
        );

        let generation = 7u64;
        let active = Arc::new(AtomicU64::new(generation));
        let (event_tx, event_rx) = unbounded();

        run_content_worker(
            generation,
            tmp.path.clone(),
            "main".to_string(),
            false,
            false,
            active,
            event_tx,
        );

        let mut match_rows = Vec::new();
        let mut done = None;
        for ev in event_rx.try_iter() {
            match ev {
                ContentEvent::Started { generation: g } => assert_eq!(g, generation),
                ContentEvent::MatchBatch {
                    generation: g,
                    results,
                } => {
                    assert_eq!(g, generation);
                    match_rows.extend(results);
                }
                ContentEvent::Done {
                    generation: g,
                    total_matches,
                    ..
                } => {
                    assert_eq!(g, generation);
                    done = Some(total_matches);
                }
                ContentEvent::Error { generation: g, .. } => panic!("error for gen {g}"),
            }
        }

        assert_eq!(done, Some(1));
        assert_eq!(match_rows.len(), 1);
        let m = &match_rows[0];
        assert_eq!(m.rel_path, "src/main.rs");
        assert_eq!(m.line, 1);
        assert_eq!(m.col, 4);
        assert!(m.snippet.contains("fn main()"));
        assert!(m.match_len >= 1);
    }
}
