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
        let dir =
            std::env::temp_dir().join(format!("nit-test-{prefix}-{}-{nanos}", std::process::id()));
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
    assert!(status.success(), "git {cmd:?} failed");
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(path, content).expect("write file");
}

fn contains_path(paths: &[String], rel: &str) -> bool {
    paths.iter().any(|p| p == rel)
}

#[test]
fn fuzzy_score_bytes_returns_matched_indices() {
    let hay = b"src/main.rs";
    let (score, indices) = fuzzy_score_bytes(hay, b"srs").expect("match should exist");
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
    assert!(contains_path(&paths, "src/main.rs"));
    assert!(!contains_path(&paths, ".env"));
    assert!(!contains_path(&paths, "ignored.txt"));
    assert!(!contains_path(&paths, "target/build.log"));

    let paths = list_index_paths(&tmp.path, true, false).expect("list_index_paths");
    assert!(contains_path(&paths, "src/main.rs"));
    assert!(contains_path(&paths, ".env"));
    assert!(!contains_path(&paths, "ignored.txt"));

    let paths = list_index_paths(&tmp.path, true, true).expect("list_index_paths");
    assert!(contains_path(&paths, "src/main.rs"));
    assert!(contains_path(&paths, ".env"));
    assert!(contains_path(&paths, "ignored.txt"));
    assert!(contains_path(&paths, "target/build.log"));
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
