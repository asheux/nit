use super::*;
use std::fs;
use std::time::{Duration, Instant};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let mut p = std::env::temp_dir();
        let nonce: u128 = Instant::now().elapsed().as_nanos();
        p.push(format!("nit-mp-dirsearch-{tag}-{nonce}"));
        fs::create_dir_all(&p).unwrap();
        Self { path: p }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn drain_until<F: Fn(&DirSearchEvent) -> bool>(
    rx: &Receiver<DirSearchEvent>,
    deadline: Duration,
    pred: F,
) -> Option<DirSearchEvent> {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(50)) {
            if pred(&ev) {
                return Some(ev);
            }
        }
    }
    None
}

#[test]
fn walks_one_level_deep() {
    let tmp = TempDir::new("walk1");
    fs::create_dir(tmp.path.join("alpha")).unwrap();
    fs::create_dir(tmp.path.join("beta")).unwrap();
    fs::create_dir(tmp.path.join("alpha").join("nested")).unwrap();

    let runner = DirSearchRunner::spawn();
    runner.query(tmp.path.clone(), String::new(), false, Vec::new());
    let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
        matches!(ev, DirSearchEvent::Results { .. })
    })
    .expect("results");
    let DirSearchEvent::Results { results, .. } = evt;
    let names: Vec<String> = results
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(names.contains(&"alpha".to_string()));
    assert!(names.contains(&"beta".to_string()));
    assert!(!names.contains(&"nested".to_string()));
}

#[test]
fn hidden_dirs_skipped_by_default() {
    let tmp = TempDir::new("hidden");
    fs::create_dir(tmp.path.join("visible")).unwrap();
    fs::create_dir(tmp.path.join(".hidden")).unwrap();

    let runner = DirSearchRunner::spawn();
    runner.query(tmp.path.clone(), String::new(), false, Vec::new());
    let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
        matches!(ev, DirSearchEvent::Results { .. })
    })
    .unwrap();
    let DirSearchEvent::Results { results, .. } = evt;
    let names: Vec<String> = results
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(names.contains(&"visible".to_string()));
    assert!(!names.iter().any(|n| n == ".hidden"));
}

#[test]
fn node_modules_and_target_skipped() {
    let tmp = TempDir::new("heavy");
    fs::create_dir(tmp.path.join("src")).unwrap();
    fs::create_dir(tmp.path.join("node_modules")).unwrap();
    fs::create_dir(tmp.path.join("target")).unwrap();

    let runner = DirSearchRunner::spawn();
    runner.query(tmp.path.clone(), String::new(), false, Vec::new());
    let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
        matches!(ev, DirSearchEvent::Results { .. })
    })
    .unwrap();
    let DirSearchEvent::Results { results, .. } = evt;
    let names: Vec<String> = results
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().any(|n| n == "src"));
    assert!(!names.iter().any(|n| n == "node_modules"));
    assert!(!names.iter().any(|n| n == "target"));
}

#[test]
fn show_hidden_includes_dotfiles() {
    let tmp = TempDir::new("show-hidden");
    fs::create_dir(tmp.path.join("visible")).unwrap();
    fs::create_dir(tmp.path.join(".cache")).unwrap();

    let runner = DirSearchRunner::spawn();
    runner.query(tmp.path.clone(), String::new(), true, Vec::new());
    let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
        matches!(ev, DirSearchEvent::Results { .. })
    })
    .unwrap();
    let DirSearchEvent::Results { results, .. } = evt;
    let names: Vec<String> = results
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(names.contains(&".cache".to_string()));
}

#[test]
fn cancel_supersedes_older_request() {
    let tmp = TempDir::new("cancel");
    fs::create_dir(tmp.path.join("alpha")).unwrap();

    let runner = DirSearchRunner::spawn();
    let id1 = runner.query(tmp.path.clone(), "alpha".into(), false, Vec::new());
    let id2 = runner.query(tmp.path.clone(), String::new(), false, Vec::new());
    assert!(id2 > id1);
    // Drain every event we receive within the deadline. Supersession
    // contract: anything still tagged id1 must NOT arrive (its request
    // was already invalidated by id2 by the time the worker checked the
    // active latch).
    let deadline = Duration::from_secs(2);
    let start = Instant::now();
    let mut saw_id2 = false;
    while start.elapsed() < deadline {
        match runner.events.recv_timeout(Duration::from_millis(50)) {
            Ok(DirSearchEvent::Results { request_id, .. }) => {
                assert_ne!(request_id, id1, "id1 must not be delivered");
                if request_id == id2 {
                    saw_id2 = true;
                    break;
                }
            }
            Err(_) => continue,
        }
    }
    assert!(saw_id2, "id2's results must arrive");
}

#[test]
fn missing_path_returns_empty_results() {
    let bogus = PathBuf::from("/this/path/does/not/exist/abc-xyz-nit");
    let runner = DirSearchRunner::spawn();
    runner.query(bogus.clone(), String::new(), false, Vec::new());
    let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
        matches!(ev, DirSearchEvent::Results { .. })
    })
    .expect("results");
    let DirSearchEvent::Results { results, .. } = evt;
    assert!(results.is_empty());
}

#[test]
fn ranked_results_for_needle() {
    let tmp = TempDir::new("rank");
    fs::create_dir(tmp.path.join("alpha")).unwrap();
    fs::create_dir(tmp.path.join("alphabet")).unwrap();
    fs::create_dir(tmp.path.join("beta")).unwrap();

    let runner = DirSearchRunner::spawn();
    runner.query(tmp.path.clone(), "alp".into(), false, Vec::new());
    let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
        matches!(ev, DirSearchEvent::Results { .. })
    })
    .unwrap();
    let DirSearchEvent::Results { results, .. } = evt;
    let names: Vec<String> = results
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().any(|n| n == "alpha"));
    assert!(names.iter().any(|n| n == "alphabet"));
    assert!(!names.iter().any(|n| n == "beta"));
}

#[test]
fn walker_skips_gitignored_dir() {
    let tmp = TempDir::new("gitignore");
    fs::create_dir(tmp.path.join("src")).unwrap();
    fs::create_dir(tmp.path.join("target_local")).unwrap();
    fs::create_dir(tmp.path.join("vendor")).unwrap();

    let runner = DirSearchRunner::spawn();
    runner.query(
        tmp.path.clone(),
        String::new(),
        false,
        vec!["target_local".into(), "vendor".into()],
    );
    let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
        matches!(ev, DirSearchEvent::Results { .. })
    })
    .expect("results");
    let DirSearchEvent::Results { results, .. } = evt;
    let names: Vec<String> = results
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().any(|n| n == "src"));
    assert!(!names.iter().any(|n| n == "target_local"));
    assert!(!names.iter().any(|n| n == "vendor"));
}

#[test]
fn recursive_finds_nested_match() {
    let tmp = TempDir::new("recursive");
    fs::create_dir_all(tmp.path.join("Myproject/foo/myproject1")).unwrap();
    fs::create_dir_all(tmp.path.join("other/sibling")).unwrap();

    let runner = DirSearchRunner::spawn();
    runner.query(tmp.path.clone(), "mypro".into(), false, Vec::new());
    let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
        matches!(ev, DirSearchEvent::Results { .. })
    })
    .expect("results");
    let DirSearchEvent::Results { results, .. } = evt;
    let rels: Vec<String> = results
        .iter()
        .map(|p| relative_haystack(&tmp.path, p))
        .collect();
    assert!(
        rels.iter().any(|r| r == "Myproject"),
        "expected Myproject in {rels:?}"
    );
    assert!(
        rels.iter().any(|r| r == "Myproject/foo/myproject1"),
        "expected nested myproject1 in {rels:?}"
    );
    assert!(
        !rels.iter().any(|r| r.contains("sibling")),
        "non-matching dirs must not appear: {rels:?}"
    );
}

#[test]
fn expanded_dir_inlines_children() {
    let tmp = TempDir::new("expanded");
    fs::create_dir_all(tmp.path.join("a/x")).unwrap();
    fs::create_dir_all(tmp.path.join("a/y")).unwrap();
    fs::create_dir(tmp.path.join("b")).unwrap();

    let mut expanded = HashSet::new();
    expanded.insert(tmp.path.join("a"));

    let runner = DirSearchRunner::spawn();
    runner.query_with_expanded(tmp.path.clone(), String::new(), false, Vec::new(), expanded);
    let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
        matches!(ev, DirSearchEvent::Results { .. })
    })
    .expect("results");
    let DirSearchEvent::Results { results, .. } = evt;
    let rels: Vec<String> = results
        .iter()
        .map(|p| relative_haystack(&tmp.path, p))
        .collect();
    assert_eq!(rels, vec!["a", "a/x", "a/y", "b"]);
}

#[test]
fn walker_caps_depth_at_max_depth() {
    let tmp = TempDir::new("depth");
    // Build a chain past MAX_DEPTH so the deepest entry is past
    // the cap. The walker must not surface MAX_DEPTH+1 segments.
    let mut chain = tmp.path.clone();
    for i in 0..(MAX_DEPTH + 4) {
        chain = chain.join(format!("d{i}"));
        fs::create_dir(&chain).unwrap();
    }

    let runner = DirSearchRunner::spawn();
    runner.query(
        tmp.path.clone(),
        format!("d{}", MAX_DEPTH + 3),
        false,
        Vec::new(),
    );
    let evt = drain_until(&runner.events, Duration::from_secs(2), |ev| {
        matches!(ev, DirSearchEvent::Results { .. })
    })
    .expect("results");
    let DirSearchEvent::Results { results, .. } = evt;
    for p in &results {
        let rel = relative_haystack(&tmp.path, p);
        let segs = rel.split('/').count();
        assert!(
            segs <= MAX_DEPTH,
            "found {rel} with {segs} segments — max is {MAX_DEPTH}"
        );
    }
}

#[test]
fn breadcrumb_uses_forward_slashes() {
    let tmp = TempDir::new("breadcrumb");
    fs::create_dir_all(tmp.path.join("alpha/beta")).unwrap();
    let nested = tmp.path.join("alpha/beta");
    let rel = relative_haystack(&tmp.path, &nested);
    assert_eq!(rel, "alpha/beta");
    assert!(!rel.contains('\\'), "must not contain platform sep");
}
