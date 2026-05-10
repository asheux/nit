use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::cache::{delete_genome_report, load_genome_reports, persist_genome_report};
use super::migrations::gc_genome_cache;
use super::schema::{genome_dir, report_path, MAX_CACHE_AGE_SECS, MAX_CACHE_BYTES};
use crate::genome_report::{GenomeReport, GenomeTier, ParsimonyInfo};
use crate::genome_report_cache::{count_at_or_above, tier_histogram, GenomeReportMap};
use crate::test_helpers::temp_dir;

fn sample_report(file_path: &Path) -> GenomeReport {
    GenomeReport {
        file_path: file_path.to_path_buf(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.5,
        tier: GenomeTier::Spaceship,
        recommendations: Vec::new(),
        timestamp_ms: 1_700_000_000_000,
        grid_size: 32,
        parsimony: ParsimonyInfo::default(),
    }
}

fn sample_report_at(file_path: &Path, timestamp_ms: u64) -> GenomeReport {
    let mut r = sample_report(file_path);
    r.timestamp_ms = timestamp_ms;
    r
}

fn set_mtime(path: &Path, time: SystemTime) {
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for set_modified");
    file.set_modified(time).expect("set_modified");
}

#[test]
fn report_paths_spread_across_shards() {
    let ws = temp_dir("genome-shard");
    for i in 0..1000 {
        let source = ws.join(format!("crates/widget_{i}.rs"));
        persist_genome_report(&ws, &sample_report(&source));
    }
    let dir = genome_dir(&ws);
    let shards: HashSet<_> = fs::read_dir(&dir)
        .expect("read genome dir")
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|ft| ft.is_dir()))
        .map(|e| e.file_name())
        .collect();
    assert!(
        shards.len() > 200,
        "expected wide shard distribution, got {} shards",
        shards.len()
    );
}

#[test]
fn round_trip_persists_and_loads() {
    let ws = temp_dir("genome-roundtrip");
    let source = ws.join("crates/example/src/lib.rs");
    let report = sample_report(&source);

    persist_genome_report(&ws, &report);
    let loaded = load_genome_reports(&ws);

    let got = loaded.get(&source).expect("report present after persist");
    assert_eq!(got.file_path, source);
    assert_eq!(got.tier, GenomeTier::Spaceship);
    assert_eq!(got.timestamp_ms, report.timestamp_ms);
}

#[test]
fn delete_removes_persisted_report() {
    let ws = temp_dir("genome-delete");
    let source = ws.join("crates/example/src/main.rs");

    persist_genome_report(&ws, &sample_report(&source));
    assert!(load_genome_reports(&ws).contains_key(&source));

    delete_genome_report(&ws, &source);
    assert!(!load_genome_reports(&ws).contains_key(&source));
}

#[test]
fn load_ignores_temp_sidecar_from_partial_write() {
    let ws = temp_dir("genome-partial");
    let source = ws.join("crates/example/src/sidecar.rs");
    persist_genome_report(&ws, &sample_report(&source));

    let report_file = report_path(&ws, &source);
    let shard_dir = report_file.parent().expect("report has parent shard dir");
    let mut tmp_name = report_file.file_name().expect("filename").to_os_string();
    tmp_name.push(".tmp.99999.7");
    let tmp_sibling = shard_dir.join(tmp_name);
    fs::write(&tmp_sibling, b"{ this is not valid json").expect("write sidecar");

    let loaded = load_genome_reports(&ws);
    assert_eq!(loaded.len(), 1, "tmp sibling must not be parsed");
    assert!(loaded.contains_key(&source));
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

#[test]
fn tier_histogram_counts_per_ladder_position() {
    let mut map = GenomeReportMap::new();
    let tiers = [
        GenomeTier::StillLife,
        GenomeTier::Oscillator,
        GenomeTier::Spaceship,
        GenomeTier::Spaceship,
        GenomeTier::Methuselah,
        GenomeTier::Methuselah,
        GenomeTier::Methuselah,
        GenomeTier::Replicator,
    ];
    for (i, tier) in tiers.iter().enumerate() {
        let path = PathBuf::from(format!("file_{i}.rs"));
        let mut report = sample_report(&path);
        report.tier = *tier;
        map.insert(path, report);
    }

    assert_eq!(tier_histogram(&map), [1, 1, 2, 3, 1]);
    assert_eq!(count_at_or_above(&map, GenomeTier::Spaceship), 6);
    assert_eq!(count_at_or_above(&map, GenomeTier::Replicator), 1);
}

#[test]
fn collision_suffix_disambiguates_flattened_paths() {
    let ws = temp_dir("genome-collision");
    let nested = PathBuf::from("a/b/foo.rs");
    let flat = PathBuf::from("a__b/foo.rs");

    persist_genome_report(&ws, &sample_report_at(&nested, 100));
    persist_genome_report(&ws, &sample_report_at(&flat, 200));

    let p_nested = report_path(&ws, &nested);
    let p_flat = report_path(&ws, &flat);
    assert_ne!(p_nested, p_flat, "encoded basenames must not collide");

    let loaded = load_genome_reports(&ws);
    assert_eq!(loaded.get(&nested).map(|r| r.timestamp_ms), Some(100));
    assert_eq!(loaded.get(&flat).map(|r| r.timestamp_ms), Some(200));
}
