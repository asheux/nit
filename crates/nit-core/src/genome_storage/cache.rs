//! Read/write hot path for the file-per-report cache.
//!
//! Writes go through [`nit_utils::fs::write_atomic`] so a crash mid-write
//! leaves a `.tmp.<pid>.<n>` sibling rather than a half-written report; the
//! load path filters by extension and silently skips any non-json siblings.
//! `write_atomic` does not fsync the parent directory after rename — accepted
//! for a re-derivable cache (worst case: miss → recompute).

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

use nit_utils::fs::write_atomic;

use super::errors::CacheError;
use super::schema::{genome_dir, report_path, REPORT_EXTENSION};
use crate::genome_report::GenomeReport;

pub fn persist_genome_report(workspace_root: &Path, report: &GenomeReport) {
    let _ = try_persist(workspace_root, report);
}

fn try_persist(workspace_root: &Path, report: &GenomeReport) -> Result<(), CacheError> {
    let path = report_path(workspace_root, &report.file_path);
    let parent = path.parent().ok_or(CacheError::MissingParent)?;
    fs::create_dir_all(parent)?;
    write_atomic(&path, |writer| {
        serde_json::to_writer(writer, report).map_err(io::Error::other)
    })?;
    Ok(())
}

pub fn delete_genome_report(workspace_root: &Path, file_path: &Path) {
    let _ = fs::remove_file(report_path(workspace_root, file_path));
}

pub fn load_genome_reports(workspace_root: &Path) -> HashMap<PathBuf, GenomeReport> {
    let dir = genome_dir(workspace_root);
    let Ok(shards) = fs::read_dir(&dir) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for shard in shards.flatten() {
        let Ok(meta) = shard.file_type() else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(shard.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(report) = read_report(&entry.path()) {
                out.insert(report.file_path.clone(), report);
            }
        }
    }
    out
}

fn read_report(path: &Path) -> Option<GenomeReport> {
    if path.extension().and_then(|e| e.to_str()) != Some(REPORT_EXTENSION) {
        return None;
    }
    let file = File::open(path).ok()?;
    serde_json::from_reader(BufReader::new(file)).ok()
}
