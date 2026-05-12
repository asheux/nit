use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use super::sample_report;
use crate::genome_report::GenomeTier;
use crate::genome_storage::cache::{
    delete_genome_report, load_genome_reports, persist_genome_report,
};
use crate::genome_storage::schema::{genome_dir, report_path};
use crate::test_helpers::temp_dir;

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
fn collision_suffix_disambiguates_flattened_paths() {
    let ws = temp_dir("genome-collision");
    let nested = PathBuf::from("a/b/foo.rs");
    let flat = PathBuf::from("a__b/foo.rs");

    let mut nested_report = sample_report(&nested);
    nested_report.timestamp_ms = 100;
    let mut flat_report = sample_report(&flat);
    flat_report.timestamp_ms = 200;
    persist_genome_report(&ws, &nested_report);
    persist_genome_report(&ws, &flat_report);

    let p_nested = report_path(&ws, &nested);
    let p_flat = report_path(&ws, &flat);
    assert_ne!(p_nested, p_flat, "encoded basenames must not collide");

    let loaded = load_genome_reports(&ws);
    assert_eq!(loaded.get(&nested).map(|r| r.timestamp_ms), Some(100));
    assert_eq!(loaded.get(&flat).map(|r| r.timestamp_ms), Some(200));
}
