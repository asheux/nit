use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::sample_report;
use crate::genome_storage::cache::{load_genome_reports, persist_genome_report};
use crate::genome_storage::migrations::gc_genome_cache;
use crate::genome_storage::schema::{genome_dir, report_path, MAX_CACHE_AGE_SECS, MAX_CACHE_BYTES};
use crate::test_helpers::temp_dir;

fn set_mtime(path: &Path, time: SystemTime) {
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for set_modified");
    file.set_modified(time).expect("set_modified");
}

#[test]
fn gc_drops_reports_older_than_max_age() {
    let ws = temp_dir("genome-gc-age");
    let source = ws.join("crates/example/src/old.rs");
    persist_genome_report(&ws, &sample_report(&source));

    let report_file = report_path(&ws, &source);
    let stale = SystemTime::now() - Duration::from_secs(MAX_CACHE_AGE_SECS + 60);
    set_mtime(&report_file, stale);

    gc_genome_cache(&ws);
    assert!(load_genome_reports(&ws).is_empty());
}

#[test]
fn gc_trims_to_byte_ceiling_evicting_oldest_first() {
    let ws = temp_dir("genome-gc-bytes");
    let dir = genome_dir(&ws);
    let shard = dir.join("00");
    fs::create_dir_all(&shard).expect("create shard dir");

    let payload = vec![b'x'; (MAX_CACHE_BYTES as usize / 3) + 1];
    let now = SystemTime::now();
    let day = Duration::from_secs(60 * 60 * 24);
    let names: Vec<PathBuf> = (0..4)
        .map(|i| shard.join(format!("forged-{i}.json")))
        .collect();
    for (i, path) in names.iter().enumerate() {
        fs::write(path, &payload).expect("write forged report");
        set_mtime(path, now - day * (4 - i as u32));
    }

    gc_genome_cache(&ws);

    let surviving: u64 = fs::read_dir(&shard)
        .expect("read shard")
        .flatten()
        .filter_map(|e| e.metadata().ok().map(|m| m.len()))
        .sum();
    assert!(
        surviving <= MAX_CACHE_BYTES,
        "surviving bytes {surviving} exceed cap {MAX_CACHE_BYTES}"
    );
    assert!(!names[0].exists(), "oldest forged file should be evicted");
    assert!(names[3].exists(), "newest forged file should be retained");
}

#[test]
fn gc_removes_legacy_flat_layout_and_unknown_versions() {
    let ws = temp_dir("genome-gc-legacy");
    let genome_root = ws.join(".nit").join("genome");
    fs::create_dir_all(genome_root.join("v0")).expect("create v0");
    fs::write(genome_root.join("v0/relic.json"), b"{}").expect("write v0 relic");
    fs::write(genome_root.join("flat.json"), b"{}").expect("write flat relic");
    persist_genome_report(&ws, &sample_report(&ws.join("keep.rs")));

    gc_genome_cache(&ws);

    assert!(!genome_root.join("v0").exists());
    assert!(!genome_root.join("flat.json").exists());
    assert!(!load_genome_reports(&ws).is_empty());
}
